//! Desktop control tools for screen capture and input simulation
//!
//! Provides capabilities for:
//! - Screen capture (screenshots)
//! - Mouse control (move, click, scroll)
//! - Keyboard control (type, press, hotkeys)
//! - Application launching

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use image::ImageBuffer;
use image::RgbImage;

/// Desktop tool for screen capture and control
#[derive(Clone)]
pub struct DesktopTool {
    config: DesktopConfig,
}

/// Configuration for desktop control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// Require approval for control actions (click, type, etc.)
    pub require_control_approval: bool,
    /// Screenshot capture is automatic (no approval needed)
    pub auto_screenshot: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            require_control_approval: true,
            auto_screenshot: true,
        }
    }
}

impl DesktopTool {
    /// Create a new desktop tool with default configuration
    pub fn new() -> Self {
        Self {
            config: DesktopConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: DesktopConfig) -> Self {
        Self { config }
    }

    /// Capture a screenshot of the desktop
    /// Returns base64-encoded PNG image
    pub fn capture_screenshot(&self) -> Result<ScreenshotResult> {
        use screenshots::Screen;
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        // Get all screens
        let screens = Screen::all().context("Failed to get screen list")?;

        if screens.is_empty() {
            bail!("No screens found");
        }

        // Capture the primary screen (first one)
        let screen = screens.into_iter().next().context("No primary screen available")?;

        // Capture the screen
        let image = screen.capture().context("Failed to capture screenshot")?;

        // Convert to PNG bytes using the image crate
        let width = image.width();
        let height = image.height();
        let raw_data = image.as_raw();

        // Create an image buffer from the RGBA data
        let img_buffer: ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(width, height, raw_data.clone())
                .context("Failed to create image buffer")?;

        // Encode as PNG
        let mut png_bytes: Vec<u8> = Vec::new();
        img_buffer.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .context("Failed to encode screenshot as PNG")?;

        let base64_data = STANDARD.encode(&png_bytes);

        Ok(ScreenshotResult {
            width,
            height,
            base64_data,
            media_type: "image/png".to_string(),
        })
    }

    /// Capture a specific region of the screen
    pub fn capture_region(&self, x: i32, y: i32, width: u32, height: u32) -> Result<ScreenshotResult> {
        use screenshots::Screen;
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let screens = Screen::all().context("Failed to get screen list")?;
        let screen = screens.into_iter().next().context("No screen available")?;

        // Capture the specified region
        let image = screen.capture_area(x, y, width, height)
            .context("Failed to capture screen region")?;

        let img_width = image.width();
        let img_height = image.height();
        let raw_data = image.as_raw();

        // Create an image buffer from the RGBA data
        let img_buffer: ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(img_width, img_height, raw_data.clone())
                .context("Failed to create image buffer")?;

        // Encode as PNG
        let mut png_bytes: Vec<u8> = Vec::new();
        img_buffer.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .context("Failed to encode screenshot as PNG")?;

        let base64_data = STANDARD.encode(&png_bytes);

        Ok(ScreenshotResult {
            width: img_width,
            height: img_height,
            base64_data,
            media_type: "image/png".to_string(),
        })
    }
}

// ============================================================================
// Mouse Control
// ============================================================================

impl DesktopTool {
    /// Move the mouse cursor to a position
    pub fn mouse_move(&self, x: i32, y: i32) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.move_mouse(x, y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.move_mouse(x, y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.move_mouse(x, y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
        }

        Ok(())
    }

    /// Click the mouse at a position
    pub fn mouse_click(&self, x: Option<i32>, y: Option<i32>, button: MouseButton) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            // Move to position if specified
            if let (Some(px), Some(py)) = (x, y) {
                enigo.move_mouse(px, py, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            }

            // Perform the click
            let btn = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            };
            enigo.button(btn, enigo::Direction::Click).context("Failed to click")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            if let (Some(px), Some(py)) = (x, y) {
                enigo.move_mouse(px, py, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            }

            let btn = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            };
            enigo.button(btn, enigo::Direction::Click).context("Failed to click")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            if let (Some(px), Some(py)) = (x, y) {
                enigo.move_mouse(px, py, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            }

            let btn = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            };
            enigo.button(btn, enigo::Direction::Click).context("Failed to click")?;
        }

        Ok(())
    }

    /// Double-click the mouse
    pub fn mouse_double_click(&self, x: Option<i32>, y: Option<i32>) -> Result<()> {
        // Double-click by clicking twice
        self.mouse_click(x, y, MouseButton::Left)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.mouse_click(x, y, MouseButton::Left)?;
        Ok(())
    }

    /// Scroll the mouse wheel
    pub fn mouse_scroll(&self, direction: ScrollDirection, amount: i32) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            let scroll_amount = match direction {
                ScrollDirection::Up => -amount,
                ScrollDirection::Down => amount,
            };
            enigo.scroll(scroll_amount, enigo::Axis::Vertical).context("Failed to scroll")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            let scroll_amount = match direction {
                ScrollDirection::Up => -amount,
                ScrollDirection::Down => amount,
            };
            enigo.scroll(scroll_amount, enigo::Axis::Vertical).context("Failed to scroll")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Mouse};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            let scroll_amount = match direction {
                ScrollDirection::Up => -amount,
                ScrollDirection::Down => amount,
            };
            enigo.scroll(scroll_amount, enigo::Axis::Vertical).context("Failed to scroll")?;
        }

        Ok(())
    }

    /// Drag from one position to another
    pub fn mouse_drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            // Move to start position
            enigo.move_mouse(from_x, from_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;

            // Press and hold
            enigo.button(Button::Left, enigo::Direction::Press).context("Failed to press button")?;

            // Move to end position
            enigo.move_mouse(to_x, to_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;

            // Release
            enigo.button(Button::Left, enigo::Direction::Release).context("Failed to release button")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            enigo.move_mouse(from_x, from_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            enigo.button(Button::Left, enigo::Direction::Press).context("Failed to press button")?;
            enigo.move_mouse(to_x, to_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            enigo.button(Button::Left, enigo::Direction::Release).context("Failed to release button")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Mouse, Button};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            enigo.move_mouse(from_x, from_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            enigo.button(Button::Left, enigo::Direction::Press).context("Failed to press button")?;
            enigo.move_mouse(to_x, to_y, enigo::Coordinate::Abs).context("Failed to move mouse")?;
            enigo.button(Button::Left, enigo::Direction::Release).context("Failed to release button")?;
        }

        Ok(())
    }
}

// ============================================================================
// Keyboard Control
// ============================================================================

impl DesktopTool {
    /// Type text using the keyboard
    pub fn keyboard_type(&self, text: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Keyboard};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.text(text).context("Failed to type text")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Keyboard};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.text(text).context("Failed to type text")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Keyboard};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            enigo.text(text).context("Failed to type text")?;
        }

        Ok(())
    }

    /// Press a single key
    pub fn keyboard_press(&self, key: Key) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            let enigo_key = self.key_to_enigo(key);
            enigo.key(enigo_key, enigo::Direction::Click).context("Failed to press key")?;
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            let enigo_key = self.key_to_enigo(key);
            enigo.key(enigo_key, enigo::Direction::Click).context("Failed to press key")?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;
            let enigo_key = self.key_to_enigo(key);
            enigo.key(enigo_key, enigo::Direction::Click).context("Failed to press key")?;
        }

        Ok(())
    }

    /// Press a keyboard hotkey (combination of keys)
    pub fn keyboard_hotkey(&self, keys: &[Key]) -> Result<()> {
        if keys.is_empty() {
            bail!("No keys specified for hotkey");
        }

        #[cfg(target_os = "linux")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            // Press all keys down
            for key in keys {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Press).context("Failed to press key")?;
            }

            // Release all keys in reverse order
            for key in keys.iter().rev() {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Release).context("Failed to release key")?;
            }
        }

        #[cfg(target_os = "macos")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            for key in keys {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Press).context("Failed to press key")?;
            }

            for key in keys.iter().rev() {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Release).context("Failed to release key")?;
            }
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::{Enigo, Settings, Keyboard, Key};
            let mut enigo = Enigo::new(&Settings::default()).context("Failed to create Enigo")?;

            for key in keys {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Press).context("Failed to press key")?;
            }

            for key in keys.iter().rev() {
                let enigo_key = self.key_to_enigo(*key);
                enigo.key(enigo_key, enigo::Direction::Release).context("Failed to release key")?;
            }
        }

        Ok(())
    }

    /// Convert our Key enum to enigo Key
    fn key_to_enigo(&self, key: Key) -> enigo::Key {
        use enigo::Key as EnigoKey;

        match key {
            Key::Enter => EnigoKey::Return,
            Key::Tab => EnigoKey::Tab,
            Key::Escape => EnigoKey::Escape,
            Key::Backspace => EnigoKey::Backspace,
            Key::Delete => EnigoKey::Delete,
            Key::Insert => EnigoKey::Insert,
            Key::Home => EnigoKey::Home,
            Key::End => EnigoKey::End,
            Key::PageUp => EnigoKey::PageUp,
            Key::PageDown => EnigoKey::PageDown,
            Key::ArrowUp => EnigoKey::UpArrow,
            Key::ArrowDown => EnigoKey::DownArrow,
            Key::ArrowLeft => EnigoKey::LeftArrow,
            Key::ArrowRight => EnigoKey::RightArrow,
            Key::F1 => EnigoKey::F1,
            Key::F2 => EnigoKey::F2,
            Key::F3 => EnigoKey::F3,
            Key::F4 => EnigoKey::F4,
            Key::F5 => EnigoKey::F5,
            Key::F6 => EnigoKey::F6,
            Key::F7 => EnigoKey::F7,
            Key::F8 => EnigoKey::F8,
            Key::F9 => EnigoKey::F9,
            Key::F10 => EnigoKey::F10,
            Key::F11 => EnigoKey::F11,
            Key::F12 => EnigoKey::F12,
            Key::Ctrl => EnigoKey::Control,
            Key::Alt => EnigoKey::Alt,
            Key::Shift => EnigoKey::Shift,
            Key::Meta => EnigoKey::Meta,
            Key::Space => EnigoKey::Space,
            Key::Char(c) => EnigoKey::Unicode(c),
        }
    }
}

// ============================================================================
// Application Control
// ============================================================================

impl DesktopTool {
    /// Open an application by name
    pub fn open_application(&self, name: &str) -> Result<()> {
        // Use system command to launch application
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{} &", name))
                .spawn()
                .context("Failed to launch application")?;
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-a")
                .arg(name)
                .spawn()
                .context("Failed to launch application")?;
        }

        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", name])
                .spawn()
                .context("Failed to launch application")?;
        }

        Ok(())
    }
}

// ============================================================================
// Data Types
// ============================================================================

/// Result of a screenshot capture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Base64-encoded PNG data
    pub base64_data: String,
    /// MIME type (always "image/png")
    pub media_type: String,
}

/// Mouse button types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Scroll direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Keyboard key types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Ctrl,
    Alt,
    Shift,
    Meta,
    Space,
    #[serde(rename = "char")]
    Char(char),
}

impl Key {
    /// Parse a key name string to Key enum
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "enter" | "return" => Some(Key::Enter),
            "tab" => Some(Key::Tab),
            "escape" | "esc" => Some(Key::Escape),
            "backspace" => Some(Key::Backspace),
            "delete" | "del" => Some(Key::Delete),
            "insert" => Some(Key::Insert),
            "home" => Some(Key::Home),
            "end" => Some(Key::End),
            "pageup" | "page_up" => Some(Key::PageUp),
            "pagedown" | "page_down" => Some(Key::PageDown),
            "up" | "arrow_up" => Some(Key::ArrowUp),
            "down" | "arrow_down" => Some(Key::ArrowDown),
            "left" | "arrow_left" => Some(Key::ArrowLeft),
            "right" | "arrow_right" => Some(Key::ArrowRight),
            "f1" => Some(Key::F1),
            "f2" => Some(Key::F2),
            "f3" => Some(Key::F3),
            "f4" => Some(Key::F4),
            "f5" => Some(Key::F5),
            "f6" => Some(Key::F6),
            "f7" => Some(Key::F7),
            "f8" => Some(Key::F8),
            "f9" => Some(Key::F9),
            "f10" => Some(Key::F10),
            "f11" => Some(Key::F11),
            "f12" => Some(Key::F12),
            "ctrl" | "control" => Some(Key::Ctrl),
            "alt" | "option" => Some(Key::Alt),
            "shift" => Some(Key::Shift),
            "meta" | "cmd" | "command" | "super" | "win" => Some(Key::Meta),
            "space" => Some(Key::Space),
            s if s.len() == 1 => Some(Key::Char(s.chars().next().unwrap())),
            _ => None,
        }
    }
}

impl Default for DesktopTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_from_name() {
        assert_eq!(Key::from_name("enter"), Some(Key::Enter));
        assert_eq!(Key::from_name("ENTER"), Some(Key::Enter));
        assert_eq!(Key::from_name("tab"), Some(Key::Tab));
        assert_eq!(Key::from_name("ctrl"), Some(Key::Ctrl));
        assert_eq!(Key::from_name("a"), Some(Key::Char('a')));
        assert_eq!(Key::from_name("invalid"), None);
    }

    #[test]
    fn test_desktop_config_default() {
        let config = DesktopConfig::default();
        assert!(config.require_control_approval);
        assert!(config.auto_screenshot);
    }
}