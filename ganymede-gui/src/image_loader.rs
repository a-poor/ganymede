//! Raster image downloads. Server credentials are only used for Contents requests.
use ganymede_core::jupyter::{JupyterRestClient, ServerConfig};
use gpui::Image;
use std::sync::Arc;

pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub type ImageResult = Result<Arc<Image>, String>;
pub type Images = std::collections::HashMap<String, Option<ImageResult>>;

#[derive(Debug, PartialEq)]
enum Source {
    Remote(reqwest::Url),
    Server(String),
}
fn resolve(source: &str, notebook: &str) -> Result<Source, String> {
    if source.starts_with("//") {
        return Err("Use an explicit http:// or https:// URL".into());
    }
    if let Ok(url) = reqwest::Url::parse(source) {
        return if matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
        {
            Ok(Source::Remote(url))
        } else {
            Err("Unsupported image URL".into())
        };
    }
    if source.is_empty() || source.starts_with('#') {
        return Err("Missing image path".into());
    }
    let mut base = reqwest::Url::parse("https://notebook.invalid/").unwrap();
    base.path_segments_mut()
        .unwrap()
        .extend(notebook.split('/'));
    let url = base.join(source).map_err(|_| "Invalid image path")?;
    let path = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .map_err(|_| "Invalid image path")?;
    let path = path.trim_start_matches('/');
    if path
        .split('/')
        .any(|part| matches!(part, "." | "..") || part.contains('\\') || part.contains('\0'))
    {
        return Err("Invalid image path".into());
    }
    Ok(Source::Server(path.to_owned()))
}

pub async fn load(
    source: &str,
    notebook: &str,
    config: Result<ServerConfig, String>,
) -> ImageResult {
    let bytes = match resolve(source, notebook)? {
        Source::Remote(url) => {
            // Do not attach server authorization, cookies, or referrers to external requests.
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(|_| "Could not create image client")?;
            let mut response = client
                .get(url)
                .send()
                .await
                .map_err(|_| "Image request failed")?;
            if !response.status().is_success() {
                return Err(format!(
                    "Image returned HTTP {}",
                    response.status().as_u16()
                ));
            }
            if response
                .content_length()
                .is_some_and(|len| len > MAX_BYTES as u64)
            {
                return Err("Image exceeds 32 MB".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "Image download failed")?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                    return Err("Image exceeds 32 MB".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        }
        Source::Server(path) => {
            JupyterRestClient::new(config.map_err(|_| "No Jupyter server configured")?)
                .map_err(|_| "Could not create Jupyter client")?
                .read_file_bytes(&path, MAX_BYTES)
                .await
                .map_err(|_| "Could not load image from Jupyter")?
        }
    };
    decode(bytes)
}
fn decode(bytes: Vec<u8>) -> ImageResult {
    use gpui::ImageFormat;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| "Invalid image")?;
    let format = match reader.format() {
        Some(image::ImageFormat::Png) => ImageFormat::Png,
        Some(image::ImageFormat::Jpeg) => ImageFormat::Jpeg,
        Some(image::ImageFormat::Gif) => ImageFormat::Gif,
        Some(image::ImageFormat::WebP) => ImageFormat::Webp,
        _ => return Err("Unsupported image format (use PNG, JPEG, GIF, or WebP)".into()),
    };
    let (w, h) = reader.into_dimensions().map_err(|_| "Invalid image")?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 32_000_000 {
        return Err("Image dimensions exceed preview limit".into());
    }
    Ok(Arc::new(Image::from_bytes(format, bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_notebook_relative_paths_without_treating_urls_as_server_files() {
        assert_eq!(
            resolve("../images/a%20b.png", "notes/demo.ipynb").unwrap(),
            Source::Server("images/a b.png".into())
        );
        assert_eq!(
            resolve("plot.png", "notes/demo.ipynb").unwrap(),
            Source::Server("notes/plot.png".into())
        );
        assert_eq!(
            resolve("/images/a.png", "notes/demo.ipynb").unwrap(),
            Source::Server("images/a.png".into())
        );
        assert!(matches!(
            resolve("http://example.test/a.png", "demo.ipynb"),
            Ok(Source::Remote(_))
        ));
        for source in [
            "file:///tmp/a.png",
            "data:image/png;base64,abc",
            "javascript:x",
            "//example.test/a.png",
            "https://user:secret@example.test/a.png",
            "%2e%2e%2fa.png",
        ] {
            assert!(resolve(source, "demo.ipynb").is_err(), "{source}");
        }
    }
    #[test]
    fn rejects_non_image_responses() {
        assert!(decode(b"<html>login</html>".to_vec()).is_err());
    }
    #[tokio::test]
    async fn remote_images_load_without_jupyter_credentials_and_reject_oversized_responses() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let server = tokio::spawn(async move {
            for oversized in [false, true] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut buf = [0; 2048];
                    let count = socket.read(&mut buf).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buf[..count]);
                }
                assert!(
                    !String::from_utf8(request)
                        .unwrap()
                        .to_ascii_lowercase()
                        .contains("authorization:")
                );
                let length = if oversized { MAX_BYTES + 1 } else { png.len() };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                if !oversized {
                    socket.write_all(&png).await.unwrap();
                }
            }
        });
        let config = ServerConfig::new("http://jupyter.invalid/")
            .unwrap()
            .with_token("must-not-leak")
            .unwrap();
        let url = format!("http://{address}/image.png");
        assert!(load(&url, "demo.ipynb", Ok(config.clone())).await.is_ok());
        assert!(
            load(&url, "demo.ipynb", Ok(config))
                .await
                .unwrap_err()
                .contains("32 MB")
        );
        server.await.unwrap();
    }
}
