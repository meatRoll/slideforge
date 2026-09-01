//! Command-line interface for the `slideforge` binary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::Result;
use crate::hash;
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
    /// Compile a PPTD project into a `.pptx` package.
    ///
    /// Overwrite guard: an existing output file may only be overwritten when
    /// its current hash (computed live) matches the value recorded in the
    /// project's `.sync.hash` sidecar when this project last wrote it;
    /// otherwise the build is refused (exit code 2) instead of destroying
    /// unknown edits. Every successful build refreshes the record.
    Build {
        /// Path to the `.pptd` main entry file.
        file: PathBuf,
        /// Output `.pptx` path; defaults to `<entry basename>.pptx`.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Reverse-compile an existing `.pptx` package into a PPTD project.
    ///
    /// The source hash is compared against the `.sync.hash` records in the
    /// output directory: when it matches the last sync point the conversion
    /// is skipped ("unchanged → skipped convert", exit code 0) and the
    /// existing PPTD is left untouched. To start over, delete the output
    /// directory (which removes the sidecar) and convert again.
    Convert {
        /// Input `.pptx` package.
        file: PathBuf,
        /// Output directory for the PPTD project (created if missing).
        output: PathBuf,
    },
}

impl Cli {
    /// Execute the parsed command line and return the process exit code.
    pub fn run(self) -> i32 {
        match self.command {
            Command::Check { file } => run_check(&file),
            Command::Dump { file } => run_dump(&file),
            Command::Build { file, output } => run_build(&file, output.as_deref()),
            Command::Convert { file, output } => run_convert(&file, &output),
        }
    }
}

fn run_convert(input: &Path, out_dir: &Path) -> i32 {
    match pptx::import::convert_pptx_to_pptd(input, out_dir) {
        Ok(report) => {
            if report.skipped_unchanged {
                println!(
                    "unchanged → skipped convert ({} already in sync with {})",
                    out_dir.join("deck.pptd").display(),
                    input.display()
                );
                println!(
                    "  hint: the existing PPTD reflects this exact .pptx; edit it directly. To start over, delete the output directory and convert again."
                );
                if report.migrated_sidecars {
                    println!("  note: legacy .src.hash/.build.hash records migrated to .sync.hash");
                }
                return 0;
            }
            println!(
                "converted {} → {}",
                input.display(),
                out_dir.join("deck.pptd").display()
            );
            println!(
                "  {} page(s), {} element(s), {} media file(s)",
                report.page_count, report.element_count, report.media_count
            );
            if report.skipped.is_empty() {
                println!("  no unsupported constructs");
            } else {
                let mut by_reason = BTreeMap::<String, usize>::new();
                for skip in &report.skipped {
                    *by_reason.entry(skip.reason.clone()).or_default() += 1;
                    eprintln!(
                        "  skipped: page {} `{}` — {}",
                        skip.page, skip.name, skip.reason
                    );
                }
                let total: usize = report.skipped.len();
                println!("  {total} unsupported construct(s):");
                for (reason, count) in &by_reason {
                    println!("    - {count}x {reason}");
                }
            }
            if let Some(hash) = &report.src_hash {
                println!("  sync point recorded: {} = {hash}", input.display());
            }
            if report.migrated_sidecars {
                println!("  note: legacy .src.hash/.build.hash records migrated to .sync.hash");
            }
            0
        }
        Err(err) => {
            eprintln!("error: {err}");
            1
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

    // Overwrite guard: an existing output file may only be clobbered with
    // positive evidence that its current bytes are accounted for: its live
    // hash must match the value recorded when this project last legitimately
    // wrote it (convert or build). Anything else — no record, a record for a
    // different file, or a hash mismatch — refuses: overwriting a file the
    // PPTD cannot vouch for could silently destroy external edits.
    if output_path.exists() {
        let verdict = match hash::classify_output(&output_path, &project.root_dir) {
            Ok(verdict) => verdict,
            Err(err) => {
                eprintln!("error: {err}");
                return 1;
            }
        };
        use hash::OutputGuard::{InSync, Stale, Uncovered};
        match verdict {
            InSync => {}
            Stale => {
                eprintln!(
                    "refusing to build: {} changed after its last recorded sync point (external edit?) and these changes are NOT in the PPTD",
                    output_path.display()
                );
                eprintln!("  building now would permanently overwrite them.");
                eprintln!(
                    "  fix: run `slideforge convert {} {}` first to absorb the external changes,",
                    output_path.display(),
                    project.root_dir.display()
                );
                eprintln!(
                    "  or delete {} first if discarding them is intended, then build again.",
                    output_path.display()
                );
                return 2;
            }
            Uncovered => {
                eprintln!(
                    "refusing to build: {} already exists but no sync point covers it — its provenance is unknown",
                    output_path.display()
                );
                eprintln!(
                    "  overwriting it could destroy external edits that are not in the PPTD."
                );
                eprintln!(
                    "  fix: run `slideforge convert {} {}` first to make it the editable baseline,",
                    output_path.display(),
                    project.root_dir.display()
                );
                eprintln!("  or delete/rename it, or choose a different --output path.");
                return 2;
            }
        }
    }

    match pptx::writer::PptxWriter::new(&project).build(&output_path) {
        Ok(()) => {
            println!("wrote {}", output_path.display());
            // Bookkeeping: the freshly written output becomes the recorded
            // sync state for this file. That single write both authorizes
            // the next build over the same path (the A/C-mode iterate loop)
            // and makes a later `convert` of this file skip as unchanged —
            // no flag needed, every legitimate write records itself.
            match hash::sha256_of(&output_path) {
                Ok(h) => {
                    let mut records = hash::Records::load(&project.root_dir);
                    records.set(&output_path, &h);
                    match records.save(&project.root_dir) {
                        Ok(()) => {
                            println!("  sync point recorded: {} = {h}", output_path.display())
                        }
                        Err(err) => eprintln!("warning: could not record sync point: {err}"),
                    }
                }
                Err(err) => eprintln!("warning: could not hash the output for bookkeeping: {err}"),
            }
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
