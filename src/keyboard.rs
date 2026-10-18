//! Bounded named keys shared by the CLI parser and Chromium transport.

pub const UNSUPPORTED_KEY_MESSAGE: &str = "Use Enter, Tab, Escape, arrows, Home, End, PageUp, PageDown, Backspace, Delete or Space. Shift+ supports Enter, Tab, arrows, Home, End and PageUp/PageDown. Editing shortcuts: Meta+A, Meta+Z, Meta+Shift+Z, Meta+arrows, Meta+Shift+arrows, Meta+Backspace; Alt+ArrowLeft/ArrowRight, Alt+Shift+ArrowLeft/ArrowRight, Alt+Backspace and Alt+Delete.";

// Input.dispatchKeyEvent modifier bits; no separate modifier-key events.
const ALT: u8 = 1;
const META: u8 = 4;
const SHIFT: u8 = 8;
const ALT_SHIFT: u8 = ALT | SHIFT;
const META_SHIFT: u8 = META | SHIFT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Enter,
    Tab,
    Escape,
    ArrowLeft,
    ArrowUp,
    ArrowRight,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Space,
    A,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    key: Key,
    modifiers: u8,
}

impl KeyPress {
    pub fn parse(value: &str) -> Option<Self> {
        let (modifiers, name) = if let Some(name) = value.strip_prefix("Meta+") {
            (META, name)
        } else if let Some(name) = value.strip_prefix("Alt+") {
            (ALT, name)
        } else {
            (0, value)
        };
        let (modifiers, name) = name
            .strip_prefix("Shift+")
            .map_or((modifiers, name), |name| (modifiers | SHIFT, name));
        let key = match name {
            "Enter" => Key::Enter,
            "Tab" => Key::Tab,
            "Escape" => Key::Escape,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowUp" => Key::ArrowUp,
            "ArrowRight" => Key::ArrowRight,
            "ArrowDown" => Key::ArrowDown,
            "Home" => Key::Home,
            "End" => Key::End,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Space" => Key::Space,
            "A" => Key::A,
            "Z" => Key::Z,
            _ => return None,
        };
        let supported = match (modifiers, key) {
            (0, Key::A | Key::Z) => false,
            (0, _) => true,
            (
                SHIFT,
                Key::Enter
                | Key::Tab
                | Key::ArrowLeft
                | Key::ArrowUp
                | Key::ArrowRight
                | Key::ArrowDown
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown,
            ) => true,
            (
                META,
                Key::A
                | Key::Z
                | Key::ArrowLeft
                | Key::ArrowUp
                | Key::ArrowRight
                | Key::ArrowDown
                | Key::Backspace,
            ) => true,
            (
                META_SHIFT,
                Key::Z | Key::ArrowLeft | Key::ArrowUp | Key::ArrowRight | Key::ArrowDown,
            ) => true,
            (ALT, Key::ArrowLeft | Key::ArrowRight | Key::Backspace | Key::Delete) => true,
            (ALT_SHIFT, Key::ArrowLeft | Key::ArrowRight) => true,
            _ => false,
        };
        if !supported {
            return None;
        }
        Some(Self { key, modifiers })
    }

    pub fn canonical(self) -> &'static str {
        match (self.modifiers, self.key) {
            (0, _) => self.code(),
            (SHIFT, Key::Enter) => "Shift+Enter",
            (SHIFT, Key::Tab) => "Shift+Tab",
            (SHIFT, Key::ArrowLeft) => "Shift+ArrowLeft",
            (SHIFT, Key::ArrowUp) => "Shift+ArrowUp",
            (SHIFT, Key::ArrowRight) => "Shift+ArrowRight",
            (SHIFT, Key::ArrowDown) => "Shift+ArrowDown",
            (SHIFT, Key::Home) => "Shift+Home",
            (SHIFT, Key::End) => "Shift+End",
            (SHIFT, Key::PageUp) => "Shift+PageUp",
            (SHIFT, Key::PageDown) => "Shift+PageDown",
            (META, Key::A) => "Meta+A",
            (META, Key::Z) => "Meta+Z",
            (META_SHIFT, Key::Z) => "Meta+Shift+Z",
            (META, Key::ArrowLeft) => "Meta+ArrowLeft",
            (META, Key::ArrowUp) => "Meta+ArrowUp",
            (META, Key::ArrowRight) => "Meta+ArrowRight",
            (META, Key::ArrowDown) => "Meta+ArrowDown",
            (META_SHIFT, Key::ArrowLeft) => "Meta+Shift+ArrowLeft",
            (META_SHIFT, Key::ArrowUp) => "Meta+Shift+ArrowUp",
            (META_SHIFT, Key::ArrowRight) => "Meta+Shift+ArrowRight",
            (META_SHIFT, Key::ArrowDown) => "Meta+Shift+ArrowDown",
            (META, Key::Backspace) => "Meta+Backspace",
            (ALT, Key::ArrowLeft) => "Alt+ArrowLeft",
            (ALT, Key::ArrowRight) => "Alt+ArrowRight",
            (ALT_SHIFT, Key::ArrowLeft) => "Alt+Shift+ArrowLeft",
            (ALT_SHIFT, Key::ArrowRight) => "Alt+Shift+ArrowRight",
            (ALT, Key::Backspace) => "Alt+Backspace",
            (ALT, Key::Delete) => "Alt+Delete",
            _ => unreachable!("parse excludes unsupported modifier combinations"),
        }
    }

    pub fn code(self) -> &'static str {
        match self.key {
            Key::Enter => "Enter",
            Key::Tab => "Tab",
            Key::Escape => "Escape",
            Key::ArrowLeft => "ArrowLeft",
            Key::ArrowUp => "ArrowUp",
            Key::ArrowRight => "ArrowRight",
            Key::ArrowDown => "ArrowDown",
            Key::Home => "Home",
            Key::End => "End",
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Space => "Space",
            Key::A => "KeyA",
            Key::Z => "KeyZ",
        }
    }

    pub fn dom_key(self) -> &'static str {
        match self.key {
            Key::Space => " ",
            Key::A => "a",
            Key::Z if self.modifiers & SHIFT != 0 => "Z",
            Key::Z => "z",
            _ => self.code(),
        }
    }

    pub fn modifiers(self) -> u8 {
        self.modifiers
    }

    // Explicit macOS editing commands for rawKeyDown only; do not rely on an
    // AppKit key binding lookup with nativeVirtualKeyCode zero. Command names
    // follow Blink; shortcut mappings follow Playwright's macEditingCommands.ts.
    // Existing named keys retain their previous Chromium default handling.
    pub fn editing_commands(self) -> &'static [&'static str] {
        match self.canonical() {
            "Meta+A" => &["SelectAll"],
            "Meta+Z" => &["Undo"],
            "Meta+Shift+Z" => &["Redo"],
            "Meta+ArrowLeft" => &["MoveToLeftEndOfLine"],
            "Meta+ArrowRight" => &["MoveToRightEndOfLine"],
            "Meta+ArrowUp" => &["MoveToBeginningOfDocument"],
            "Meta+ArrowDown" => &["MoveToEndOfDocument"],
            "Meta+Shift+ArrowLeft" => &["MoveToLeftEndOfLineAndModifySelection"],
            "Meta+Shift+ArrowRight" => &["MoveToRightEndOfLineAndModifySelection"],
            "Meta+Shift+ArrowUp" => &["MoveToBeginningOfDocumentAndModifySelection"],
            "Meta+Shift+ArrowDown" => &["MoveToEndOfDocumentAndModifySelection"],
            "Meta+Backspace" => &["DeleteToBeginningOfLine"],
            "Alt+ArrowLeft" => &["MoveWordLeft"],
            "Alt+ArrowRight" => &["MoveWordRight"],
            "Alt+Shift+ArrowLeft" => &["MoveWordLeftAndModifySelection"],
            "Alt+Shift+ArrowRight" => &["MoveWordRightAndModifySelection"],
            "Alt+Backspace" => &["DeleteWordBackward"],
            "Alt+Delete" => &["DeleteWordForward"],
            _ => &[],
        }
    }

    // Chromium 156 ui/events/keycodes/keyboard_codes_posix.h. Native macOS
    // key codes are deliberately separate; the transport always sends zero.
    pub fn windows_virtual_key_code(self) -> u8 {
        match self.key {
            Key::Backspace => 0x08,
            Key::Tab => 0x09,
            Key::Enter => 0x0D,
            Key::Escape => 0x1B,
            Key::Space => 0x20,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::End => 0x23,
            Key::Home => 0x24,
            Key::ArrowLeft => 0x25,
            Key::ArrowUp => 0x26,
            Key::ArrowRight => 0x27,
            Key::ArrowDown => 0x28,
            Key::Delete => 0x2E,
            Key::A => 0x41,
            Key::Z => 0x5A,
        }
    }

    pub fn text(self) -> Option<&'static str> {
        match self.key {
            Key::Enter => Some("\r"),
            Key::Space => Some(" "),
            _ => None,
        }
    }
}
