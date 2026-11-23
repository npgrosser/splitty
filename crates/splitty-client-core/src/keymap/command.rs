//! The registry: every command a key can run, with its stable id, title,
//! category and default keys per platform.
use super::{Chord, Platform};

/// A neighbor of the focused pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The groups the commands are listed in, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Screens,
    Tabs,
    Panes,
    Terminal,
    View,
    Window,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Self::Screens => "Screens",
            Self::Tabs => "Tabs",
            Self::Panes => "Panes",
            Self::Terminal => "Terminal",
            Self::View => "View",
            Self::Window => "Window",
        }
    }
}

/// A command of the window. The declaration order is the registry's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    NewScreen,
    NextScreen,
    PreviousScreen,
    /// Screen 1 to 9 of the Drawer's order.
    SelectScreen(u8),
    CloseScreen,
    RenameScreen,
    MoveScreenUp,
    MoveScreenDown,
    /// Hand the shown Screen to a new window of the application.
    MoveScreenToNewWindow,
    /// Hand the shown Screen to the application's next window.
    MoveScreenToNextWindow,
    /// Hand every Screen of the shown Screen's group (the local Screens, or
    /// those of its host) to a new window.
    MoveScreenGroupToNewWindow,
    /// Hand that group to the application's next window.
    MoveScreenGroupToNextWindow,
    NewTab,
    NewTabWithProfile,
    CloseTab,
    CloseOtherTabs,
    NextTab,
    PreviousTab,
    MoveTabRight,
    MoveTabLeft,
    MoveTabToScreen,
    SplitRight,
    SplitDown,
    FocusPane(Direction),
    MaximizePane,
    FloatPane,
    ArrangePanes,
    ToggleScratchpad,
    Find,
    Copy,
    Paste,
    ClearBuffer,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    CopyWorkingDirectory,
    OpenSelectionInViewer,
    ToggleDrawer,
    FocusDrawer,
    Hosts,
    Templates,
    Settings,
    CommandPalette,
    KeyboardShortcuts,
    ConnectHost,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleFullScreen,
    /// Another window of the application, with a new local Screen.
    NewWindow,
    /// Close the window; its Screens go to another window while there is one.
    CloseWindow,
    Quit,
}

const DIRECTIONS: [Direction; 4] = [
    Direction::Left,
    Direction::Right,
    Direction::Up,
    Direction::Down,
];

impl Command {
    /// Every command, in the registry's order.
    pub fn all() -> Vec<Command> {
        use Command::*;
        let mut all = vec![NewScreen, NextScreen, PreviousScreen];
        all.extend((1..=9).map(SelectScreen));
        all.extend([
            CloseScreen,
            RenameScreen,
            MoveScreenUp,
            MoveScreenDown,
            MoveScreenToNewWindow,
            MoveScreenToNextWindow,
            MoveScreenGroupToNewWindow,
            MoveScreenGroupToNextWindow,
            NewTab,
            NewTabWithProfile,
            CloseTab,
            CloseOtherTabs,
            NextTab,
            PreviousTab,
            MoveTabRight,
            MoveTabLeft,
            MoveTabToScreen,
            SplitRight,
            SplitDown,
        ]);
        all.extend(DIRECTIONS.map(FocusPane));
        all.extend([
            MaximizePane,
            FloatPane,
            ArrangePanes,
            ToggleScratchpad,
            Find,
            Copy,
            Paste,
            ClearBuffer,
            ScrollPageUp,
            ScrollPageDown,
            ScrollTop,
            ScrollBottom,
            CopyWorkingDirectory,
            OpenSelectionInViewer,
            ToggleDrawer,
            FocusDrawer,
            Hosts,
            Templates,
            Settings,
            CommandPalette,
            KeyboardShortcuts,
            ConnectHost,
            ZoomIn,
            ZoomOut,
            ZoomReset,
            ToggleFullScreen,
            NewWindow,
            CloseWindow,
            Quit,
        ]);
        all
    }

    /// The stable id a keybindings file stores, such as `screen.select.3`.
    pub fn id(self) -> String {
        use Command::*;
        let id = match self {
            SelectScreen(index) => return format!("screen.select.{index}"),
            FocusPane(direction) => {
                return format!(
                    "pane.focus-{}",
                    match direction {
                        Direction::Left => "left",
                        Direction::Right => "right",
                        Direction::Up => "up",
                        Direction::Down => "down",
                    }
                )
            }
            NewScreen => "screen.new",
            NextScreen => "screen.next",
            PreviousScreen => "screen.previous",
            CloseScreen => "screen.close",
            RenameScreen => "screen.rename",
            MoveScreenUp => "screen.move-up",
            MoveScreenDown => "screen.move-down",
            MoveScreenToNewWindow => "screen.move-to-new-window",
            MoveScreenToNextWindow => "screen.move-to-next-window",
            MoveScreenGroupToNewWindow => "screen.move-group-to-new-window",
            MoveScreenGroupToNextWindow => "screen.move-group-to-next-window",
            NewTab => "tab.new",
            NewTabWithProfile => "tab.new-with-profile",
            CloseTab => "tab.close",
            CloseOtherTabs => "tab.close-others",
            NextTab => "tab.next",
            PreviousTab => "tab.previous",
            MoveTabRight => "tab.move-right",
            MoveTabLeft => "tab.move-left",
            MoveTabToScreen => "tab.move-to-screen",
            SplitRight => "pane.split-right",
            SplitDown => "pane.split-down",
            MaximizePane => "pane.maximize",
            FloatPane => "pane.float",
            ArrangePanes => "pane.arrange",
            ToggleScratchpad => "pane.scratchpad",
            Find => "terminal.find",
            Copy => "terminal.copy",
            Paste => "terminal.paste",
            ClearBuffer => "terminal.clear",
            ScrollPageUp => "terminal.scroll-page-up",
            ScrollPageDown => "terminal.scroll-page-down",
            ScrollTop => "terminal.scroll-top",
            ScrollBottom => "terminal.scroll-bottom",
            CopyWorkingDirectory => "terminal.copy-directory",
            OpenSelectionInViewer => "terminal.open-viewer",
            ToggleDrawer => "drawer.toggle",
            FocusDrawer => "drawer.focus",
            Hosts => "view.hosts",
            Templates => "view.templates",
            Settings => "view.settings",
            CommandPalette => "view.palette",
            KeyboardShortcuts => "view.keyboard",
            ConnectHost => "host.connect",
            ZoomIn => "zoom.in",
            ZoomOut => "zoom.out",
            ZoomReset => "zoom.reset",
            ToggleFullScreen => "window.fullscreen",
            NewWindow => "window.new",
            CloseWindow => "window.close",
            Quit => "window.quit",
        };
        id.to_owned()
    }

    /// The command a stored id names.
    pub fn from_id(id: &str) -> Option<Command> {
        Self::all().into_iter().find(|command| command.id() == id)
    }

    /// The name shown in the palette and the settings.
    pub fn title(self) -> String {
        use Command::*;
        let title = match self {
            SelectScreen(index) => return format!("Switch to Screen {index}"),
            FocusPane(direction) => {
                return format!(
                    "Focus Pane {}",
                    match direction {
                        Direction::Left => "Left",
                        Direction::Right => "Right",
                        Direction::Up => "Above",
                        Direction::Down => "Below",
                    }
                )
            }
            NewScreen => "New Screen",
            NextScreen => "Next Screen",
            PreviousScreen => "Previous Screen",
            CloseScreen => "Close Screen",
            RenameScreen => "Rename Screen",
            MoveScreenUp => "Move Screen Up",
            MoveScreenDown => "Move Screen Down",
            MoveScreenToNewWindow => "Move Screen to New Window",
            MoveScreenToNextWindow => "Move Screen to Next Window",
            MoveScreenGroupToNewWindow => "Move Screen Group to New Window",
            MoveScreenGroupToNextWindow => "Move Screen Group to Next Window",
            NewTab => "New Tab",
            NewTabWithProfile => "New Tab with Profile…",
            CloseTab => "Close Tab",
            CloseOtherTabs => "Close Other Tabs",
            NextTab => "Next Tab",
            PreviousTab => "Previous Tab",
            MoveTabRight => "Move Tab Right",
            MoveTabLeft => "Move Tab Left",
            MoveTabToScreen => "Move Tab to Screen…",
            SplitRight => "Split Right",
            SplitDown => "Split Down",
            MaximizePane => "Maximize Pane",
            FloatPane => "Float or Dock Pane",
            ArrangePanes => "Arrange Panes",
            ToggleScratchpad => "Toggle Scratchpad",
            Find => "Find in Terminal",
            Copy => "Copy",
            Paste => "Paste",
            ClearBuffer => "Clear Buffer",
            ScrollPageUp => "Scroll Page Up",
            ScrollPageDown => "Scroll Page Down",
            ScrollTop => "Scroll to Top",
            ScrollBottom => "Scroll to Bottom",
            CopyWorkingDirectory => "Copy Working Directory",
            OpenSelectionInViewer => "Open Selection in Viewer",
            ToggleDrawer => "Toggle Drawer",
            FocusDrawer => "Focus Drawer",
            Hosts => "Hosts and Ports",
            Templates => "Templates…",
            Settings => "Settings",
            CommandPalette => "Command Palette",
            KeyboardShortcuts => "Keyboard Shortcuts",
            ConnectHost => "Connect to Host…",
            ZoomIn => "Zoom In",
            ZoomOut => "Zoom Out",
            ZoomReset => "Reset Zoom",
            ToggleFullScreen => "Toggle Full Screen",
            NewWindow => "New Window",
            CloseWindow => "Close Window",
            Quit => "Quit",
        };
        title.to_owned()
    }

    pub fn category(self) -> Category {
        use Command::*;
        match self {
            NewScreen
            | NextScreen
            | PreviousScreen
            | SelectScreen(_)
            | CloseScreen
            | RenameScreen
            | MoveScreenUp
            | MoveScreenDown
            | MoveScreenToNewWindow
            | MoveScreenToNextWindow
            | MoveScreenGroupToNewWindow
            | MoveScreenGroupToNextWindow => Category::Screens,
            NewTab | NewTabWithProfile | CloseTab | CloseOtherTabs | NextTab | PreviousTab
            | MoveTabRight | MoveTabLeft | MoveTabToScreen => Category::Tabs,
            SplitRight | SplitDown | FocusPane(_) | MaximizePane | FloatPane | ArrangePanes
            | ToggleScratchpad => Category::Panes,
            Find
            | Copy
            | Paste
            | ClearBuffer
            | ScrollPageUp
            | ScrollPageDown
            | ScrollTop
            | ScrollBottom
            | CopyWorkingDirectory
            | OpenSelectionInViewer => Category::Terminal,
            ToggleDrawer | FocusDrawer | Hosts | Templates | Settings | CommandPalette
            | KeyboardShortcuts | ConnectHost => Category::View,
            ZoomIn | ZoomOut | ZoomReset | ToggleFullScreen | NewWindow | CloseWindow | Quit => {
                Category::Window
            }
        }
    }
}

/// The keys a command has until the user changes them; `mod` is Cmd on macOS
/// and Alt elsewhere.
pub fn defaults(platform: Platform, command: Command) -> Vec<Chord> {
    default_texts(platform, command)
        .iter()
        .map(|text| Chord::parse(text).expect("a default key combination"))
        .collect()
}

fn default_texts(platform: Platform, command: Command) -> Vec<String> {
    use Command::*;
    let mac = platform.is_mac();
    let primary = if mac { "cmd" } else { "alt" };
    let with = |rest: &str| vec![format!("{primary}+{rest}")];
    let fixed =
        |texts: &[&str]| -> Vec<String> { texts.iter().map(|text| (*text).to_owned()).collect() };
    let either = |other: &[&str], on_mac: &[&str]| fixed(if mac { on_mac } else { other });
    match command {
        NewScreen => fixed(&["ctrl+shift+space"]),
        NextScreen => fixed(&["ctrl+tab"]),
        PreviousScreen => fixed(&["ctrl+shift+tab"]),
        SelectScreen(index) if mac => vec![format!("cmd+{index}")],
        SelectScreen(index) => vec![format!("alt+{index}"), format!("alt+shift+{index}")],
        CloseScreen => with("shift+w"),
        RenameScreen => with("shift+r"),
        MoveScreenUp
        | MoveScreenDown
        | MoveScreenToNewWindow
        | MoveScreenToNextWindow
        | MoveScreenGroupToNewWindow
        | MoveScreenGroupToNextWindow
        | CloseWindow
        | CloseOtherTabs
        | CopyWorkingDirectory
        | OpenSelectionInViewer
        | ConnectHost => Vec::new(),
        NewTab => with("t"),
        NewTabWithProfile => with("shift+t"),
        CloseTab => with("w"),
        NextTab => with("pagedown"),
        PreviousTab => with("pageup"),
        MoveTabRight => with("shift+pagedown"),
        MoveTabLeft => with("shift+pageup"),
        MoveTabToScreen => with("shift+m"),
        SplitRight => with("d"),
        SplitDown => with("shift+d"),
        FocusPane(direction) => with(match direction {
            Direction::Left => "shift+left",
            Direction::Right => "shift+right",
            Direction::Up => "shift+up",
            Direction::Down => "shift+down",
        }),
        MaximizePane => with("m"),
        FloatPane => with("shift+f"),
        ArrangePanes => with("r"),
        ToggleScratchpad => with("j"),
        Find => either(&["ctrl+shift+f"], &["cmd+f"]),
        Copy => either(&["ctrl+shift+c", "super+c"], &["cmd+c"]),
        Paste => either(&["ctrl+shift+v", "super+v"], &["cmd+v"]),
        ClearBuffer => with("k"),
        ScrollPageUp => fixed(&["shift+pageup"]),
        ScrollPageDown => fixed(&["shift+pagedown"]),
        ScrollTop => fixed(&["shift+home"]),
        ScrollBottom => fixed(&["shift+end"]),
        ToggleDrawer => with("shift+b"),
        FocusDrawer => with("e"),
        Hosts => either(&["alt+h"], &["cmd+shift+h"]),
        Templates => with("p"),
        Settings => either(&["ctrl+,"], &["cmd+,"]),
        CommandPalette => with("shift+p"),
        KeyboardShortcuts => with("shift+k"),
        ZoomIn => either(&["ctrl+=", "ctrl+plus"], &["cmd+=", "cmd+plus"]),
        ZoomOut => either(&["ctrl+-"], &["cmd+-"]),
        ZoomReset => either(&["ctrl+0"], &["cmd+0"]),
        ToggleFullScreen => either(&["f11"], &["ctrl+cmd+f"]),
        NewWindow => either(&["ctrl+shift+n"], &["cmd+n"]),
        Quit => either(&[], &["cmd+q"]),
    }
}
