//! External launchers: editor, terminal.

use fxfm_core::config::EditorConfig;
use std::path::Path;
use std::process::Command;

/// POSIX single-quoting: the result is one shell word that means exactly `s`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Shell line run in a terminal: `cd <dir> && <cmd>`.
fn shell_line(dir: &Path, cmd: &str) -> String {
    format!("cd {} && {}", shell_quote(&dir.to_string_lossy()), cmd)
}

/// Runs `line` in a new Terminal.app window and brings Terminal to the front.
#[cfg(target_os = "macos")]
fn terminal_run(line: &str) -> Command {
    // The shell line goes inside an AppleScript string literal.
    let escaped = line.replace('\\', r"\\").replace('"', "\\\"");
    let mut c = Command::new("osascript");
    c.arg("-e")
        .arg(format!(
            "tell application \"Terminal\" to do script \"{escaped}\""
        ))
        .arg("-e")
        .arg("tell application \"Terminal\" to activate");
    c
}

/// Runs `line` in `$TERMINAL` (fallback `x-terminal-emulator`).
#[cfg(not(target_os = "macos"))]
fn terminal_run(line: &str) -> Command {
    let term = std::env::var("TERMINAL").unwrap_or_else(|_| "x-terminal-emulator".into());
    let mut c = Command::new(term);
    c.arg("-e").arg("sh").arg("-c").arg(line);
    c
}

/// Command that opens `file` in the configured editor.
pub fn editor_command(cfg: &EditorConfig, file: &Path) -> Command {
    match cfg {
        EditorConfig::Terminal { command } => {
            let dir = file.parent().unwrap_or(Path::new("/"));
            let line = format!(
                "{} {}",
                shell_line(dir, command),
                shell_quote(&file.to_string_lossy())
            );
            terminal_run(&line)
        }
        #[cfg(target_os = "macos")]
        EditorConfig::System => {
            let mut c = Command::new("open");
            if Path::new("/Applications/Visual Studio Code.app").exists() {
                c.arg("-a").arg("Visual Studio Code");
            } else {
                c.arg("-e");
            }
            c.arg(file);
            c
        }
        #[cfg(target_os = "macos")]
        EditorConfig::App { path } => {
            let mut c = Command::new("open");
            c.arg("-a").arg(path).arg(file);
            c
        }
        #[cfg(not(target_os = "macos"))]
        EditorConfig::System => {
            let mut c = Command::new("xdg-open");
            c.arg(file);
            c
        }
        #[cfg(not(target_os = "macos"))]
        EditorConfig::App { path } => {
            let mut c = Command::new(path);
            c.arg(file);
            c
        }
    }
}

/// Command that runs `cmd` in a new terminal window, in `dir`.
pub fn terminal_command(dir: &Path, cmd: &str) -> Command {
    terminal_run(&shell_line(dir, cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxfm_core::config::EditorConfig;
    use std::path::Path;

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("it's 'ok'.sh"), r#"'it'\''s '\''ok'\''.sh'"#);
    }

    #[test]
    fn editor_command_terminal_quotes_path() {
        let cfg = EditorConfig::Terminal {
            command: "vim".into(),
        };
        let cmd = editor_command(&cfg, Path::new("/tmp/my dir/a b.txt"));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let all = args.join(" ");
        assert!(all.contains("vim '/tmp/my dir/a b.txt'"), "{all}");
        assert!(all.contains("cd '/tmp/my dir'"), "{all}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn editor_command_terminal_escapes_applescript() {
        let cfg = EditorConfig::Terminal {
            command: "vim".into(),
        };
        let cmd = editor_command(&cfg, Path::new(r#"/tmp/a"b\c.txt"#));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args[1],
            r#"tell application "Terminal" to do script "cd '/tmp' && vim '/tmp/a\"b\\c.txt'""#
        );
        assert_eq!(args[3], r#"tell application "Terminal" to activate"#);
    }
}
