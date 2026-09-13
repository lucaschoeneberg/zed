use anyhow::{Context as _, Result};
use mermaid_render::{MermaidTheme, render_to_svg};

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let source_path = arguments.next().context("Expected a Mermaid source file")?;
    let destination = arguments.next().context("Expected an SVG output file")?;
    let source = std::fs::read_to_string(source_path)?;
    std::fs::write(
        destination,
        render_to_svg(&source, &MermaidTheme::default())?,
    )?;
    Ok(())
}
