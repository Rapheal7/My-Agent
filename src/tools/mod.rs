//! Tools module

pub mod filesystem;
pub mod shell;
pub mod web;
pub mod remote;
pub mod browser;
pub mod desktop;

// Re-export commonly used filesystem types
pub use filesystem::{
    FileSystemTool,
    FileContent,
    FileInfo,
    DirectoryListing,
    FileOperationResult,
    read_file,
    write_file,
    list_directory,
    is_path_accessible,
};

// Re-export commonly used shell types
pub use shell::{
    ShellTool,
    ShellConfig,
    CommandResult,
    execute,
    execute_in_dir,
    command_exists,
};

// Re-export commonly used web types
pub use web::{
    WebTool,
    WebConfig,
    WebResult,
    SearchResult,
    fetch,
    fetch_text,
    check_url,
};

// Re-export commonly used browser types
pub use browser::{
    BrowserTool,
    BrowserConfig,
    BrowserSession,
    BrowserManager,
    ScreenshotOptions,
    ScreenshotResult,
    ScreenshotFormat,
    NavigationResult,
    ScriptResult,
};

// Re-export commonly used desktop types
pub use desktop::{
    DesktopTool,
    DesktopConfig,
    MouseButton,
    ScrollDirection,
    Key,
};
