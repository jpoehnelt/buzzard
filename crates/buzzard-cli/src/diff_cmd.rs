//! The `diff` subcommand: a semantic diff of two versions of one file.

use std::path::Path;

use buzzard_lint::diff::{self, DiffOptions, ViDiff};
use buzzard_lint::{patharg, printable, Failure};
use clap::ValueEnum;

use crate::{to_json, Outcome, Report};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// The old file and the new file, or a member path for each. A file
    /// that does not exist, or /dev/null, counts as empty, so buzzard
    /// reports everything in the other file as added or removed.
    ///
    /// With --git, these are the arguments that git passes to an external
    /// diff driver.
    #[arg(required = true, num_args = 1..=9, value_name = "FILE")]
    files: Vec<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Leave out nodes that only changed position or size.
    #[arg(long)]
    ignore_moves: bool,
    /// Run as a git external diff driver. buzzard then expects git's seven
    /// arguments (path old-file old-hex old-mode new-file new-hex
    /// new-mode). The exit status is always 0, as git requires.
    #[arg(long)]
    git: bool,
}

impl Args {
    /// Whether the caller asked for a format that a program reads.
    pub(crate) fn machine(&self) -> bool {
        matches!(self.format, Format::Json)
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Markdown,
    Json,
}

fn render(result: &ViDiff, format: Format) -> String {
    match format {
        Format::Text => diff::to_text(result),
        Format::Markdown => diff::to_markdown(result),
        Format::Json => to_json(result),
    }
}

/// Returns the report and the exit status: 0 when the files are the same,
/// 1 when they differ.
pub(crate) fn run(args: &Args) -> Outcome {
    let options = DiffOptions {
        ignore_moves: args.ignore_moves,
    };
    if args.git {
        return Ok(Report::ok(git_driver(args, options)?));
    }
    let [old, new] = args.files.as_slice() else {
        return Err(Failure::usage(
            "expected two files: buzzard diff <OLD> <NEW>",
        ));
    };
    let (old, new) = (Path::new(old), Path::new(new));
    patharg::check(old)?;
    patharg::check(new)?;
    let result = diff::compare(old, new, options)?;
    Ok(Report {
        text: render(&result, args.format),
        status: u8::from(!result.identical),
    })
}

/// The calling convention of `GIT_EXTERNAL_DIFF` and `diff.<name>.command`:
/// `path old-file old-hex old-mode new-file new-hex new-mode`, plus the new
/// path and a similarity line for a rename, or the path alone when it is
/// unmerged. Git treats any failure of the driver as fatal, so problems
/// with the files become part of the output instead.
///
/// The paths are the names of files in a repository, and git passes them
/// as they are. A name with a control character is therefore compared like
/// any other, and the header prints it on one line.
fn git_driver(args: &Args, options: DiffOptions) -> Result<String, Failure> {
    let files = &args.files;
    let path = printable::text(&files[0]);
    if files.len() == 1 {
        return Ok(format!("buzzard diff {path}\nunmerged\n"));
    }
    if files.len() != 7 && files.len() != 9 {
        return Err(Failure::usage(format!(
            "--git expects the 7 arguments of a git external diff driver, got {}",
            files.len()
        )));
    }
    let new_path = files
        .get(7)
        .map_or_else(|| path.clone(), |new| printable::text(new));
    let header = format!("buzzard diff a/{path} b/{new_path}\n");
    let body = match diff::compare(Path::new(&files[1]), Path::new(&files[4]), options) {
        Ok(result) if matches!(args.format, Format::Json) => {
            return Ok(render(&result, args.format))
        }
        Ok(result) => render(&result, args.format),
        Err(failure) => format!("cannot compare: {failure}\n"),
    };
    Ok(header + &body)
}
