//! The `diff` subcommand: a semantic diff of two versions of one file.

use std::io::ErrorKind;
use std::path::Path;

use buzzard_lint::diff::{self, DiffOptions, ViDiff};
use buzzard_lint::Inspection;
use buzzard_rsrc::RsrcError;
use clap::ValueEnum;

#[derive(clap::Args)]
pub(crate) struct Args {
    /// The old and the new file. A file that does not exist (or
    /// `/dev/null`) counts as empty: everything is added or removed.
    ///
    /// With `--git`: the arguments git passes to an external diff driver.
    #[arg(required = true, num_args = 1..=9, value_name = "FILE")]
    files: Vec<String>,
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Leave out nodes that only changed position or size.
    #[arg(long)]
    ignore_moves: bool,
    /// Run as a git external diff driver. Expects git's seven arguments
    /// (path old-file old-hex old-mode new-file new-hex new-mode) and
    /// always exits with status 0, as git requires.
    #[arg(long)]
    git: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Markdown,
    Json,
}

/// One side of the comparison: the parsed file, or `None` with an optional
/// note when it does not exist or may not be opened. Anything else that
/// goes wrong (not an RSRC file, a truncated file) is an error.
type Side = (Option<Inspection>, Option<String>);

/// Whether `path` names the null device, which git passes for the side of
/// an added or deleted file: `/dev/null`, or `nul` on Windows.
fn is_null_device(path: &str) -> bool {
    path == "/dev/null" || (cfg!(windows) && path.eq_ignore_ascii_case("nul"))
}

fn load(path: &str, same_bytes: bool) -> Result<Side, String> {
    if is_null_device(path) {
        return Ok((None, None));
    }
    // A member of an archive that is not in it is like a file that is not
    // on disk: the other side was added or deleted.
    if let Some((archive, name)) = buzzard_lint::archive::split(Path::new(path)) {
        return match buzzard_lint::archive::read_member(&archive, &name) {
            Ok(Some(bytes)) => match Inspection::from_bytes(&bytes) {
                Ok(inspection) => Ok((Some(inspection), None)),
                Err(err) => Err(format!("{path}: {err}")),
            },
            Ok(None) => Ok((None, None)),
            Err(err) => Err(format!("{path}: {err}")),
        };
    }
    // Two archives are not compared member by member, and what is not
    // compared is not called the same: only two archives with the same
    // bytes are. An archive without a directory (one built into an
    // application) is a resource file like any other.
    let file = Path::new(path);
    if !same_bytes && buzzard_lint::archive::is_archive(file) && file.is_file() {
        match buzzard_lint::archive::list(file) {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(format!(
                    "{path}: an archive is not compared as a whole; name one of its members on \
                     each side, as in `{path}/<member name>` (buzzard inspect lists them)"
                ));
            }
            Err(err) => return Err(format!("{path}: {err}")),
        }
    }
    let bytes = match buzzard_rsrc::read_file(Path::new(path)) {
        Ok(bytes) => bytes,
        Err(RsrcError::Io(err)) if err.kind() == ErrorKind::NotFound => return Ok((None, None)),
        Err(RsrcError::Io(err)) if err.kind() == ErrorKind::PermissionDenied => {
            return Ok((None, Some(format!("{path} cannot be read: {err}"))));
        }
        Err(err) => return Err(format!("{path}: {err}")),
    };
    match Inspection::from_bytes(&bytes) {
        Ok(inspection) => Ok((Some(inspection), None)),
        Err(err) => Err(format!("{path}: {err}")),
    }
}

fn compare(old: &str, new: &str, options: DiffOptions) -> Result<ViDiff, String> {
    let is_archive = |path: &str| {
        let path = Path::new(path);
        buzzard_lint::archive::is_archive(path) && path.is_file()
    };
    let same_bytes = is_archive(old)
        && is_archive(new)
        && matches!(
            (buzzard_rsrc::read_file(old), buzzard_rsrc::read_file(new)),
            (Ok(old), Ok(new)) if old == new
        );
    let ((old_file, old_note), (new_file, new_note)) =
        (load(old, same_bytes)?, load(new, same_bytes)?);
    if old_file.is_none() && new_file.is_none() {
        return Err(format!("neither {old} nor {new} can be read"));
    }
    let mut result = diff::diff(old_file.as_ref(), new_file.as_ref(), options);
    result.notes.extend(old_note.into_iter().chain(new_note));
    Ok(result)
}

fn render(result: &ViDiff, format: Format) -> String {
    match format {
        Format::Text => diff::to_text(result),
        Format::Markdown => diff::to_markdown(result),
        Format::Json => {
            serde_json::to_string_pretty(result).expect("a diff always serializes") + "\n"
        }
    }
}

/// Returns the report and the exit status: 0 when the files are the same,
/// 1 when they differ. An `Err` is a message for stderr and status 2.
pub(crate) fn run(args: &Args) -> Result<(String, u8), String> {
    let options = DiffOptions {
        ignore_moves: args.ignore_moves,
    };
    if args.git {
        return Ok((git_driver(args, options)?, 0));
    }
    let [old, new] = args.files.as_slice() else {
        return Err("expected two files: buzzard diff <OLD> <NEW>".to_owned());
    };
    let result = compare(old, new, options)?;
    Ok((render(&result, args.format), u8::from(!result.identical)))
}

/// The calling convention of `GIT_EXTERNAL_DIFF` and `diff.<name>.command`:
/// `path old-file old-hex old-mode new-file new-hex new-mode`, plus the new
/// path and a similarity line for a rename, or the path alone when it is
/// unmerged. Git treats any failure of the driver as fatal, so problems
/// with the files become part of the output instead.
fn git_driver(args: &Args, options: DiffOptions) -> Result<String, String> {
    let files = &args.files;
    let path = &files[0];
    if files.len() == 1 {
        return Ok(format!("buzzard diff {path}\nunmerged\n"));
    }
    if files.len() != 7 && files.len() != 9 {
        return Err(format!(
            "--git expects the 7 arguments of a git external diff driver, got {}",
            files.len()
        ));
    }
    let new_path = files.get(7).unwrap_or(path);
    let header = format!("buzzard diff a/{path} b/{new_path}\n");
    let body = match compare(&files[1], &files[4], options) {
        Ok(result) if matches!(args.format, Format::Json) => {
            return Ok(render(&result, args.format))
        }
        Ok(result) => render(&result, args.format),
        Err(message) => format!("cannot compare: {message}\n"),
    };
    Ok(header + &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_device_is_recognised_on_every_platform() {
        assert!(is_null_device("/dev/null"));
        // `nul` is the null device on Windows only; elsewhere it is an
        // ordinary file name.
        assert_eq!(is_null_device("nul"), cfg!(windows));
        assert_eq!(is_null_device("NUL"), cfg!(windows));
        assert!(!is_null_device("null.vi"));
        assert!(!is_null_device("nul.vi"));
        assert!(load("/dev/null", false).unwrap().0.is_none());
    }
}
