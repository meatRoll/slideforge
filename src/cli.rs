//! Command-line interface for the `slideforge` binary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::Result;
use crate::pptd;
use crate::pptx;

#[derive(Debug, Parser)]
#[command(
    name = "slideforge",
    version,
    about = "Edit and build PPTX files from the PPTD language",
    long_about = "SlideForge parses, validates and compiles PPTD \
                  (a YAML-based presentation DSL) into editable PPTX files."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Load a PPTD project and report every validation issue found.
    Check {
        /// Path to the `.pptd` main entry file.
        file: PathBuf,
    },
    /// Dump a summary of the parsed project (debugging aid).
    Dump {
        /// Path to the `.pptd` main entry file.
        file: PathBuf,
    },
    /// Compile a PPTD project into a `.pptx` package (not implemented yet).
    Build {
        /// Path to the `.pptd` main entry file.
        file: PathBuf,
        /// Output `.pptx` path; defaults to `<entry basename>.pptx`.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

impl Cli {
    /// Execute the parsed command line and return the process exit code.
    pub fn run(self) -> i32 {
        match self.command {
            Command::Check { file } => run_check(&file),
            Command::Dump { file } => run_dump(&file),
            Command::Build { file, output } => run_build(&file, output.as_deref()),
        }
    }
}

fn run_check(entry: &Path) -> i32 {
    let project = match load(entry) {
        Ok(project) => project,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    let issues = pptd::validate::validate_project(&project);
    if issues.is_empty() {
        println!(
            "{}: {} page(s) validated — no issues",
            entry.display(),
            project.pages.len()
        );
        0
    } else {
        for issue in &issues {
            eprintln!("- {issue}");
        }
        eprintln!("check failed: {} issue(s) found", issues.len());
        2
    }
}

fn run_dump(entry: &Path) -> i32 {
    let project = match load(entry) {
        Ok(project) => project,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    let presentation = &project.presentation;
    println!("entry:   {}", entry.display());
    println!("version: {}", presentation.version);
    println!("title:   {:?}", presentation.title);
    println!(
        "size:    {} x {}",
        presentation.size.width, presentation.size.height
    );
    match &presentation.theme {
        Some(theme) => println!(
            "theme:   {} color(s), {} textStyle(s), {} tableStyle(s)",
            theme.colors.len(),
            theme.text_styles.len(),
            theme.table_styles.len()
        ),
        None => println!("theme:   (none)"),
    }
    println!("pages:   {}", project.pages.len());
    for (i, page) in project.pages.iter().enumerate() {
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for element in &page.elements {
            *counts.entry(element.type_name()).or_default() += 1;
        }
        let summary = counts
            .iter()
            .map(|(kind, count)| format!("{count}x {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}: {:?} — {}", i + 1, page.page_type, summary);
    }
    0
}

fn run_build(entry: &Path, output: Option<&Path>) -> i32 {
    let project = match load(entry) {
        Ok(project) => project,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    let issues = pptd::validate::validate_project(&project);
    if !issues.is_empty() {
        eprintln!("refusing to build: {} issue(s) found", issues.len());
        for issue in &issues {
            eprintln!("- {issue}");
        }
        return 2;
    }

    let output_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| entry.with_extension("pptx"));
    match pptx::writer::PptxWriter::new(&project).build(&output_path) {
        Ok(()) => {
            println!("wrote {}", output_path.display());
            0
        }
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
}

fn load(entry: &Path) -> Result<pptd::Project> {
    pptd::load_project(entry)
}
