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
    Plus => "regular/plus.svg",
    Play => "regular/play.svg",
    Stop => "regular/stop.svg",
    Restart => "regular/arrow-clockwise.svg",
    Save => "regular/floppy-disk.svg",
    Scissors => "regular/scissors.svg",
    Clipboard => "regular/clipboard.svg",
    Terminal => "regular/terminal-window.svg",
    FileText => "regular/file-text.svg",
    FileCode => "regular/file-code.svg",
    Notebook => "regular/notebook.svg",
    CaretDown => "regular/caret-down.svg",
    RunAll => "regular/fast-forward.svg",

    Info => "regular/info.svg",
    CheckCircle => "regular/check-circle.svg",
    WarningCircle => "regular/warning-circle.svg",
    SpinnerGap => "regular/spinner-gap.svg",
    X => "regular/x.svg",
    FolderPlus => "regular/folder-plus.svg",
    Filter => "regular/funnel.svg",
    Upload => "regular/upload-simple.svg",
    Folder => "regular/folder.svg",
    File => "regular/file.svg",
    GitBranch => "regular/git-branch.svg",
    Cpu => "regular/cpu.svg",
    Gear => "regular/gear.svg",
    Server => "regular/hard-drives.svg",
    Sliders => "regular/sliders-horizontal.svg",
    Palette => "regular/palette.svg",
    Desktop => "regular/desktop.svg",
    Sun => "regular/sun.svg",
    Moon => "regular/moon.svg",
    Keyboard => "regular/keyboard.svg",
    CaretLeft => "regular/caret-left.svg",
    CaretRight => "regular/caret-right.svg",
    FolderOpen => "regular/folder-open.svg",
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
    /// GPUI skips painting SVGs without an explicit text color; parent text colors
    /// are not inherited by the SVG painter.
    pub fn svg(self) -> Svg {
        svg().path(self.path())
    }
}
