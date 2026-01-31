//! Silk Terminal - HTML-styled terminal with persistent shell sessions
//!
//! Silk maintains a persistent shell session that preserves environment variables,
//! executes commands, and returns output as structured data with ANSI-to-HTML conversion.
//! Interactive commands are automatically detected and spawned in a separate PTY.

use lib_signaling_protocol::SilkHtmlSpan;
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use uuid::Uuid;

/// Known interactive commands that always need a PTY
const INTERACTIVE_COMMANDS: &[&str] = &[
    "vim",
    "nvim",
    "vi",
    "nano",
    "emacs",
    "less",
    "more",
    "top",
    "htop",
    "btop",
    "man",
    "ssh",
    "fzf",
    "lazygit",
    "tig",
    "claude",
    "python",
    "python3",
    "node",
    "irb",
    "rails c",
    "psql",
    "mysql",
    "sqlite3",
    "mongosh",
    "redis-cli",
];

/// A Silk session - persistent shell for command execution
pub struct SilkSession {
    pub id: Uuid,
    pub shell: String,
    pub cwd: String,
    pub env: HashMap<String, String>,
    /// Running commands that may need input
    pub running_commands: HashMap<Uuid, RunningCommand>,
}

/// A command running within a Silk session
pub struct RunningCommand {
    pub id: Uuid,
    pub command: String,
    pub interactive: bool,
    /// For non-interactive: child process
    pub child: Option<Child>,
    /// For interactive: PTY session ID (reuses cocoon PTY infrastructure)
    pub pty_session_id: Option<Uuid>,
}

impl SilkSession {
    /// Create a new Silk session
    pub fn new(
        cwd: Option<String>,
        env: HashMap<String, String>,
        shell: Option<String>,
    ) -> Result<Self, String> {
        // Determine shell to use
        let shell = shell
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".to_string());

        // Determine working directory
        let cwd = cwd
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".to_string());

        // Verify shell exists
        if !std::path::Path::new(&shell).exists() {
            return Err(format!("Shell not found: {}", shell));
        }

        Ok(Self {
            id: Uuid::new_v4(),
            shell,
            cwd,
            env,
            running_commands: HashMap::new(),
        })
    }

    /// Check if a command is likely interactive
    pub fn is_interactive_command(command: &str) -> bool {
        let cmd_name = command.split_whitespace().next().unwrap_or("");

        // Check against known interactive commands
        for interactive in INTERACTIVE_COMMANDS {
            if cmd_name == *interactive || cmd_name.ends_with(&format!("/{}", interactive)) {
                return true;
            }
        }

        // Check for common patterns
        if command.contains(" -i") || command.contains(" --interactive") {
            return true;
        }

        false
    }

    /// Execute a command in the session
    pub fn execute(
        &mut self,
        command: &str,
        command_id: Uuid,
    ) -> Result<(bool, Option<Child>), String> {
        let interactive = Self::is_interactive_command(command);

        if interactive {
            // Mark as needing PTY, actual PTY creation happens in core.rs
            self.running_commands.insert(
                command_id,
                RunningCommand {
                    id: command_id,
                    command: command.to_string(),
                    interactive: true,
                    child: None,
                    pty_session_id: None,
                },
            );
            return Ok((true, None));
        }

        // Non-interactive: execute with piped I/O
        // We wrap in shell to properly handle pipes, redirects, etc.
        let mut cmd = Command::new(&self.shell);
        cmd.arg("-c").arg(command);
        cmd.current_dir(&self.cwd);

        // Set environment
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        // Set common terminal env vars for proper output
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("FORCE_COLOR", "1");

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        self.running_commands.insert(
            command_id,
            RunningCommand {
                id: command_id,
                command: command.to_string(),
                interactive: false,
                child: None, // We return the child, caller manages it
                pty_session_id: None,
            },
        );

        Ok((false, Some(child)))
    }

    /// Update cwd if command was a cd
    pub fn update_cwd_if_cd(&mut self, command: &str) {
        let trimmed = command.trim();
        if trimmed.starts_with("cd ") {
            let path = trimmed.strip_prefix("cd ").unwrap().trim();
            // Handle ~ expansion
            let path = if path.starts_with('~') {
                if let Ok(home) = std::env::var("HOME") {
                    path.replacen('~', &home, 1)
                } else {
                    path.to_string()
                }
            } else if path.starts_with('/') {
                path.to_string()
            } else {
                format!("{}/{}", self.cwd, path)
            };

            // Normalize path
            if let Ok(canonical) = std::fs::canonicalize(&path) {
                self.cwd = canonical.to_string_lossy().to_string();
            }
        }
    }

    /// Set PTY session ID for interactive command
    pub fn set_pty_session(&mut self, command_id: Uuid, pty_session_id: Uuid) {
        if let Some(cmd) = self.running_commands.get_mut(&command_id) {
            cmd.pty_session_id = Some(pty_session_id);
        }
    }

    /// Remove completed command
    pub fn complete_command(&mut self, command_id: Uuid) {
        self.running_commands.remove(&command_id);
    }
}

/// ANSI to HTML converter
pub struct AnsiToHtml;

impl AnsiToHtml {
    /// Convert ANSI escape codes to HTML spans
    pub fn convert(input: &str) -> Vec<SilkHtmlSpan> {
        let mut spans = Vec::new();
        let mut current_text = String::new();
        let mut current_styles: HashMap<String, String> = HashMap::new();
        let mut current_classes: Vec<String> = Vec::new();

        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                // Flush current text
                if !current_text.is_empty() {
                    spans.push(SilkHtmlSpan {
                        text: current_text.clone(),
                        classes: current_classes.clone(),
                        styles: current_styles.clone(),
                    });
                    current_text.clear();
                }

                // Parse escape sequence
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    let mut code = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_ascii_digit() || c == ';' {
                            code.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    // Consume final character (usually 'm' for SGR)
                    if let Some(final_char) = chars.next() {
                        if final_char == 'm' {
                            Self::parse_sgr(&code, &mut current_styles, &mut current_classes);
                        }
                    }
                }
            } else {
                current_text.push(ch);
            }
        }

        // Flush remaining text
        if !current_text.is_empty() {
            spans.push(SilkHtmlSpan {
                text: current_text,
                classes: current_classes,
                styles: current_styles,
            });
        }

        spans
    }

    /// Parse SGR (Select Graphic Rendition) codes
    fn parse_sgr(code: &str, styles: &mut HashMap<String, String>, classes: &mut Vec<String>) {
        if code.is_empty() || code == "0" {
            // Reset
            styles.clear();
            classes.clear();
            return;
        }

        for part in code.split(';') {
            match part {
                "1" => {
                    classes.push("bold".to_string());
                }
                "2" => {
                    classes.push("dim".to_string());
                }
                "3" => {
                    classes.push("italic".to_string());
                }
                "4" => {
                    classes.push("underline".to_string());
                }
                "7" => {
                    classes.push("inverse".to_string());
                }
                "9" => {
                    classes.push("strikethrough".to_string());
                }
                // Foreground colors
                "30" => {
                    styles.insert("color".to_string(), "#000000".to_string());
                }
                "31" => {
                    styles.insert("color".to_string(), "#cc0000".to_string());
                }
                "32" => {
                    styles.insert("color".to_string(), "#00cc00".to_string());
                }
                "33" => {
                    styles.insert("color".to_string(), "#cccc00".to_string());
                }
                "34" => {
                    styles.insert("color".to_string(), "#0000cc".to_string());
                }
                "35" => {
                    styles.insert("color".to_string(), "#cc00cc".to_string());
                }
                "36" => {
                    styles.insert("color".to_string(), "#00cccc".to_string());
                }
                "37" => {
                    styles.insert("color".to_string(), "#cccccc".to_string());
                }
                // Bright foreground colors
                "90" => {
                    styles.insert("color".to_string(), "#555555".to_string());
                }
                "91" => {
                    styles.insert("color".to_string(), "#ff5555".to_string());
                }
                "92" => {
                    styles.insert("color".to_string(), "#55ff55".to_string());
                }
                "93" => {
                    styles.insert("color".to_string(), "#ffff55".to_string());
                }
                "94" => {
                    styles.insert("color".to_string(), "#5555ff".to_string());
                }
                "95" => {
                    styles.insert("color".to_string(), "#ff55ff".to_string());
                }
                "96" => {
                    styles.insert("color".to_string(), "#55ffff".to_string());
                }
                "97" => {
                    styles.insert("color".to_string(), "#ffffff".to_string());
                }
                // Background colors
                "40" => {
                    styles.insert("background-color".to_string(), "#000000".to_string());
                }
                "41" => {
                    styles.insert("background-color".to_string(), "#cc0000".to_string());
                }
                "42" => {
                    styles.insert("background-color".to_string(), "#00cc00".to_string());
                }
                "43" => {
                    styles.insert("background-color".to_string(), "#cccc00".to_string());
                }
                "44" => {
                    styles.insert("background-color".to_string(), "#0000cc".to_string());
                }
                "45" => {
                    styles.insert("background-color".to_string(), "#cc00cc".to_string());
                }
                "46" => {
                    styles.insert("background-color".to_string(), "#00cccc".to_string());
                }
                "47" => {
                    styles.insert("background-color".to_string(), "#cccccc".to_string());
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_interactive_command() {
        assert!(SilkSession::is_interactive_command("vim"));
        assert!(SilkSession::is_interactive_command("vim file.txt"));
        assert!(SilkSession::is_interactive_command("/usr/bin/vim"));
        assert!(SilkSession::is_interactive_command("claude"));
        assert!(SilkSession::is_interactive_command("python"));
        assert!(SilkSession::is_interactive_command("python3 -i"));
        assert!(!SilkSession::is_interactive_command("ls"));
        assert!(!SilkSession::is_interactive_command("cat file.txt"));
        assert!(!SilkSession::is_interactive_command("echo hello"));
    }

    #[test]
    fn test_ansi_to_html_plain_text() {
        let spans = AnsiToHtml::convert("hello world");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "hello world");
        assert!(spans[0].classes.is_empty());
        assert!(spans[0].styles.is_empty());
    }

    #[test]
    fn test_ansi_to_html_bold() {
        let spans = AnsiToHtml::convert("\x1b[1mBOLD\x1b[0m normal");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "BOLD");
        assert!(spans[0].classes.contains(&"bold".to_string()));
        assert_eq!(spans[1].text, " normal");
        assert!(spans[1].classes.is_empty());
    }

    #[test]
    fn test_ansi_to_html_red() {
        let spans = AnsiToHtml::convert("\x1b[31mRED\x1b[0m");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "RED");
        assert_eq!(spans[0].styles.get("color"), Some(&"#cc0000".to_string()));
    }

    #[test]
    fn test_ansi_to_html_combined() {
        let spans = AnsiToHtml::convert("\x1b[1;32mBOLD GREEN\x1b[0m");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "BOLD GREEN");
        assert!(spans[0].classes.contains(&"bold".to_string()));
        assert_eq!(spans[0].styles.get("color"), Some(&"#00cc00".to_string()));
    }
}
