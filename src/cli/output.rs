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

pub struct CliOutput {
    value: Option<Box<dyn CliOutputValue>>,
    pub elapsed_duration_warn_threshold: Option<std::time::Duration>,
}

impl core::fmt::Debug for CliOutput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CliOutput")
            .field("has_value", &self.value.is_some())
            .field(
                "elapsed_duration_warn_threshold",
                &self.elapsed_duration_warn_threshold,
            )
            .finish()
    }
}

trait CliOutputValue {
    fn render(&self, format: OutputFormat, stdout_is_terminal: bool) -> eyre::Result<String>;
}

struct FacetCliOutput<T> {
    value: T,
}

struct FacetRowsCliOutput<T> {
    rows: Vec<T>,
}

impl CliOutput {
    #[must_use]
    pub const fn none(elapsed_duration_warn_threshold: Option<std::time::Duration>) -> Self {
        Self {
            value: None,
            elapsed_duration_warn_threshold,
        }
    }

    #[must_use]
    pub fn facet<T>(value: T, elapsed_duration_warn_threshold: Option<std::time::Duration>) -> Self
    where
        T: Facet<'static> + 'static,
    {
        Self {
            value: Some(Box::new(FacetCliOutput { value })),
            elapsed_duration_warn_threshold,
        }
    }

    #[must_use]
    pub fn facet_rows<T>(
        rows: Vec<T>,
        elapsed_duration_warn_threshold: Option<std::time::Duration>,
    ) -> Self
    where
        T: Facet<'static> + 'static,
    {
        Self {
            value: Some(Box::new(FacetRowsCliOutput { rows })),
            elapsed_duration_warn_threshold,
        }
    }

    /// # Errors
    ///
    /// Returns an error if rendering or writing the selected format fails.
    pub fn emit(self, requested_format: Option<OutputFormat>) -> eyre::Result<()> {
        let Some(output) = self.value else {
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

impl<T> CliOutputValue for FacetRowsCliOutput<T>
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
                .format(&self.rows)),
            OutputFormat::Json => facet_json::to_string_pretty(&self.rows)
                .wrap_err("failed to serialize command output rows as JSON"),
            OutputFormat::Csv => {
                let mut rendered = String::new();
                for row in &self.rows {
                    rendered.push_str(
                        &facet_csv::to_string(row)
                            .wrap_err("failed to serialize command output row as CSV")?,
                    );
                }
                Ok(rendered)
            }
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

    fn example_rows_output() -> FacetRowsCliOutput<ExampleReport> {
        FacetRowsCliOutput {
            rows: vec![
                ExampleReport {
                    name: String::from("first"),
                    count: 1,
                },
                ExampleReport {
                    name: String::from("second"),
                    count: 2,
                },
            ],
        }
    }

    #[test]
    fn budget_metadata_preserves_json_and_csv_rows() {
        for format in [OutputFormat::Json, OutputFormat::Csv, OutputFormat::Text] {
            let render = |threshold| {
                let receipt = CliOutput::facet_rows(example_rows_output().rows, threshold);
                receipt.value.unwrap().render(format, false).unwrap()
            };
            assert_eq!(
                render(None),
                render(Some(std::time::Duration::from_secs(1)))
            );
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

    #[test]
    fn renders_flat_rows_as_csv() {
        let rendered = example_rows_output()
            .render(OutputFormat::Csv, false)
            .expect("CSV rows should render");
        assert_eq!(rendered, "first,1\nsecond,2\n");
    }

    #[test]
    fn renders_flat_rows_as_json_and_text() {
        let json = example_rows_output()
            .render(OutputFormat::Json, false)
            .expect("JSON rows should render");
        let text = example_rows_output()
            .render(OutputFormat::Text, false)
            .expect("text rows should render");
        for rendered in [json, text] {
            assert!(rendered.contains("first"));
            assert!(rendered.contains("second"));
        }
    }
}
