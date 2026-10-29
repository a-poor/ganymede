//! Add an icon to `define_icons!` to generate its typed name and embed allowlist.

use gpui::{Svg, svg};

macro_rules! define_icons {
    ($($name:ident => $path:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($name),+
        }

        #[derive(rust_embed::RustEmbed)]
        #[folder = "assets/icons/phosphor/"]
        #[prefix = "icons/phosphor/"]
        $(#[include = $path])+
        #[include = "LICENSE"]
        pub(crate) struct IconAssets;

        // Fail at compile time for a misspelled or missing asset. Embed glob
        // filters alone would silently omit a path that does not exist.
        $(const _: &[u8] = include_bytes!(concat!("../assets/icons/phosphor/", $path));)+

        impl Icon {
            pub const ALL: &'static [Self] = &[$(Self::$name),+];

            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$name => concat!("icons/phosphor/", $path)),+
                }
            }
        }
    };
}

define_icons! {
    SquaresFour => "regular/squares-four.svg",
    Clock => "regular/clock.svg",
    SidebarSimple => "regular/sidebar-simple.svg",
    Trash => "regular/trash.svg",
    DotsThree => "regular/dots-three.svg",
    Copy => "regular/copy.svg",
    ArrowUp => "regular/arrow-up.svg",
    ArrowDown => "regular/arrow-down.svg",
    RowsPlusTop => "regular/rows-plus-top.svg",
    RowsPlusBottom => "regular/rows-plus-bottom.svg",
}

impl Icon {
    /// Set size and text color on the returned SVG using the view's theme.
    pub fn svg(self) -> Svg {
        svg().path(self.path())
    }
}
