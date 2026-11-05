#[path = "src/connection/planet_scene.rs"]
mod planet_scene;

fn main() {
    println!("cargo:rerun-if-changed=src/connection/planet_scene.rs");
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        path.join("planet-opening.bin"),
        planet_scene::Scene::new().mask(0.),
    )
    .expect("write pre-rendered planet opening frame");
}
