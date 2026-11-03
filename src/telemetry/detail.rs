use super::{CodexEvent, FieldValue};
use facet::Facet;

#[derive(Debug, Facet)]
struct ExecArguments {
    cmd: Option<String>,
    shell: Option<String>,
    workdir: Option<String>,
}

/// A missing shell is deliberately distinct from explicit PowerShell or another shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellKind {
    PowerShell(String),
    Other(String),
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandDetail {
    pub raw_arguments: Option<String>,
    pub command: Option<String>,
    pub shell: ShellKind,
    pub workdir: Option<String>,
    pub output: Option<String>,
    pub output_truncated: Option<bool>,
}

/// Extract known `exec_command` fields. Original event fields remain untouched.
#[must_use]
pub fn command_detail(event: &CodexEvent) -> CommandDetail {
    fn text(value: Option<&FieldValue>) -> Option<String> {
        match value {
            Some(FieldValue::Text(value)) => Some(value.clone()),
            _ => None,
        }
    }
    let raw_arguments = text(event.field("arguments"));
    let parsed = if matches!(event.field("tool_name"), Some(FieldValue::Text(name)) if name == "exec_command")
    {
        raw_arguments
            .as_deref()
            .and_then(|arguments| facet_json::from_str::<ExecArguments>(arguments).ok())
    } else {
        None
    };
    let shell = match parsed
        .as_ref()
        .and_then(|args| args.shell.as_deref())
        .filter(|shell| !shell.is_empty())
    {
        None => ShellKind::Unknown,
        Some(shell) => {
            let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
            if name.eq_ignore_ascii_case("powershell")
                || name.eq_ignore_ascii_case("powershell.exe")
                || name.eq_ignore_ascii_case("pwsh")
                || name.eq_ignore_ascii_case("pwsh.exe")
            {
                ShellKind::PowerShell(shell.into())
            } else {
                ShellKind::Other(shell.into())
            }
        }
    };
    CommandDetail {
        raw_arguments,
        command: parsed.as_ref().and_then(|args| args.cmd.clone()),
        shell,
        workdir: parsed.and_then(|args| args.workdir),
        output: text(event.field("output")),
        output_truncated: match event.field("output_truncated") {
            Some(FieldValue::Bool(value)) => Some(*value),
            Some(FieldValue::Text(value)) if value == "true" => Some(true),
            Some(FieldValue::Text(value)) if value == "false" => Some(false),
            _ => None,
        },
    }
}

/// Render control and bidirectional formatting characters visibly without executing text.
#[must_use]
pub fn escape_display(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        if (ch.is_control() && ch != '\n' && ch != '\t')
            || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            output.extend(ch.escape_unicode());
        } else {
            output.push(ch);
        }
    }
    output
}
