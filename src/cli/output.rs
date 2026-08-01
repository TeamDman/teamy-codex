use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use facet_pretty::ColorMode;
use facet_pretty::PrettyPrinter;
use std::io;
use std::io::IsTerminal;
use std::io::Write;

#[derive(Arbitrary, Facet, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[facet(rename_all = "kebab-case")]
#[repr(u8)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Csv,
}

pub struct CliOutput(Option<Box<dyn CliOutputValue>>);

impl core::fmt::Debug for CliOutput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CliOutput")
            .field("has_value", &self.0.is_some())
            .finish()
    }
}

trait CliOutputValue {
    fn render(&self, format: OutputFormat, stdout_is_terminal: bool) -> eyre::Result<String>;
}

struct FacetCliOutput<T> {
    value: T,
}

impl CliOutput {
    #[must_use]
    pub const fn none() -> Self {
        Self(None)
    }

    #[must_use]
    pub fn facet<T>(value: T) -> Self
    where
        T: Facet<'static> + 'static,
    {
        Self(Some(Box::new(FacetCliOutput { value })))
    }

    /// # Errors
    ///
    /// Returns an error if rendering or writing the selected format fails.
    pub fn emit(self, requested_format: Option<OutputFormat>) -> eyre::Result<()> {
        let Some(output) = self.0 else {
            return Ok(());
        };
        let stdout_is_terminal = io::stdout().is_terminal();
        let format = requested_format.unwrap_or(if stdout_is_terminal {
            OutputFormat::Text
        } else {
            OutputFormat::Json
        });
        let rendered = output.render(format, stdout_is_terminal)?;
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(rendered.as_bytes())
            .wrap_err("failed to write command output")?;
        if !rendered.ends_with('\n') {
            stdout
                .write_all(b"\n")
                .wrap_err("failed to terminate command output")?;
        }
        stdout.flush().wrap_err("failed to flush command output")?;
        Ok(())
    }
}

impl Default for CliOutput {
    fn default() -> Self {
        Self::none()
    }
}

impl<T> CliOutputValue for FacetCliOutput<T>
where
    T: Facet<'static> + 'static,
{
    fn render(&self, format: OutputFormat, stdout_is_terminal: bool) -> eyre::Result<String> {
        match format {
            OutputFormat::Text => Ok(PrettyPrinter::new()
                .with_colors(if stdout_is_terminal {
                    ColorMode::Always
                } else {
                    ColorMode::Never
                })
                .format(&self.value)),
            OutputFormat::Json => facet_json::to_string_pretty(&self.value)
                .wrap_err("failed to serialize command output as JSON"),
            OutputFormat::Csv => facet_csv::to_string(&self.value)
                .wrap_err("failed to serialize command output as CSV"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Facet, Debug)]
    struct ExampleReport {
        name: String,
        count: u32,
    }

    fn example_output() -> FacetCliOutput<ExampleReport> {
        FacetCliOutput {
            value: ExampleReport {
                name: String::from("example"),
                count: 3,
            },
        }
    }

    #[test]
    fn renders_json() {
        let rendered = example_output()
            .render(OutputFormat::Json, false)
            .expect("JSON rendering should succeed");
        assert!(rendered.contains("\"name\": \"example\""));
    }

    #[test]
    fn renders_csv() {
        let rendered = example_output()
            .render(OutputFormat::Csv, false)
            .expect("CSV rendering should succeed");
        assert!(rendered.contains("example"));
        assert!(rendered.contains('3'));
    }

    #[test]
    fn renders_text() {
        let rendered = example_output()
            .render(OutputFormat::Text, false)
            .expect("text rendering should succeed");
        assert!(rendered.contains("example"));
    }
}
