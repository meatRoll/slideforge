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
    Build {
        /// Path to the `.pptd` main entry file.
        file: PathBuf,
        /// Output `.pptx` path; defaults to `<entry basename>.pptx`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Record the output's SHA-256 as the sync point in the project's
        /// `.src.hash` sidecar. Use for in-place edit flows where the output
        /// overwrites the source `.pptx` the PPTD was converted from; the
        /// next `convert` of that file will then be skipped as unchanged.
        #[arg(long)]
        sync: bool,
    },
    /// Reverse-compile an existing `.pptx` package into a PPTD project.
    ///
    /// The source hash is compared against the `.src.hash` sidecar in the
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
            Command::Build { file, output, sync } => run_build(&file, output.as_deref(), sync),
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
                println!("  sync point recorded: .src.hash = {hash}");
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

fn run_build(entry: &Path, output: Option<&Path>, sync: bool) -> i32 {
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

    // Overwrite guard: refuse to clobber external edits. If the output
    // overwrites the file recorded as the sync-point source and that file's
    // current content no longer matches the recorded hash, the .pptx was
    // edited after the last sync point and the PPTD has not absorbed those
    // changes — building now would silently destroy them.
    let external_edit = match hash::overwrites_sync_source(&output_path, &project.root_dir) {
        true => match hash::matches_stored(&output_path, &project.root_dir) {
            Ok(matches) => !matches,
            Err(err) => {
                eprintln!("error: {err}");
                return 1;
            }
        },
        false => false,
    };
    if external_edit {
        eprintln!(
            "refusing to build: {} changed after the last sync point (external edit?) and these changes are NOT in the PPTD",
            output_path.display()
        );
        eprintln!("  building now would permanently overwrite them.");
        eprintln!(
            "  fix: run `slideforge convert {} {}` first to absorb the external changes,",
            output_path.display(),
            project.root_dir.display()
        );
        eprintln!("  then redo the PPTD edits on the fresh baseline and build again.");
        return 2;
    }

    match pptx::writer::PptxWriter::new(&project).build(&output_path) {
        Ok(()) => {
            println!("wrote {}", output_path.display());
            // Sync-point bookkeeping: record the output hash when either
            // (a) `--sync` was passed explicitly, or (b) the output
            // overwrites the file recorded as the sync-point source — i.e.
            // this build IS the next sync point, no flag needed. Without
            // this, a forgotten flag would make the next `convert`
            // misread the agent's own build as an external edit.
            let auto_sync = hash::overwrites_sync_source(&output_path, &project.root_dir);
            if sync || auto_sync {
                match hash::sha256_of(&output_path).and_then(|h| {
                    hash::write_stored(&project.root_dir, &h, Some(&output_path)).map(|_| h)
                }) {
                    Ok(h) => {
                        if auto_sync && !sync {
                            println!(
                                "  auto-sync: output overwrites the convert source; sync point recorded: .src.hash = {h}"
                            );
                        } else {
                            println!("  sync point recorded: .src.hash = {h}");
                        }
                    }
                    Err(err) => {
                        eprintln!("warning: could not record .src.hash: {err}");
                    }
                }
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
