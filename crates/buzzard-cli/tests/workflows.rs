//! What the GitHub workflows may do. The release workflow publishes, and only
//! the owner starts it. The binaries workflow builds and uploads workflow
//! artifacts, and it can change nothing in the repository.
//!
//! These tests read the YAML as text, line by line, because the workflows
//! keep to a plain layout: two spaces per level, and one key per line.

use std::fs;
use std::path::PathBuf;

const BINARIES: &str = ".github/workflows/binaries.yml";
const RELEASE: &str = ".github/workflows/release.yml";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A text file, with the `\r\n` line endings that a Windows checkout may
/// have added replaced by `\n`.
fn read(relative: &str) -> String {
    let path = root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Every workflow file, as a path relative to the root.
fn workflows() -> Vec<String> {
    let dir = root().join(".github/workflows");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
                .path()
        })
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
            })
        })
        .map(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            format!(".github/workflows/{name}")
        })
        .collect();
    names.sort();
    assert!(names.len() >= 3, "found the workflows: {names:?}");
    names
}

/// The lines of a workflow without comments and blank lines.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .collect()
}

/// The keys one level under the top-level key `key`, such as the triggers
/// under `on:` or the jobs under `jobs:`.
fn keys_under(text: &str, key: &str) -> Vec<String> {
    let header = format!("{key}:");
    let lines = code_lines(text);
    let start = lines
        .iter()
        .position(|line| *line == header)
        .unwrap_or_else(|| panic!("the workflow has no top-level `{header}`"));
    lines[start + 1..]
        .iter()
        .take_while(|line| line.starts_with(' '))
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .map(|line| line.trim().trim_end_matches(':').to_owned())
        .collect()
}

/// The lines of one job, from its name to the next job.
fn job<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("  {name}:");
    let lines = code_lines(text);
    let start = lines
        .iter()
        .position(|line| *line == header)
        .unwrap_or_else(|| panic!("the workflow has no job `{name}`"));
    lines[start + 1..]
        .iter()
        .take_while(|line| line.starts_with("   "))
        .copied()
        .collect()
}

#[test]
fn every_action_is_pinned_to_a_commit() {
    for workflow in workflows() {
        let text = read(&workflow);
        for line in text.lines() {
            let trimmed = line.trim_start().trim_start_matches("- ");
            let Some(action) = trimmed.strip_prefix("uses:") else {
                continue;
            };
            let action = action.trim();
            // A workflow of this repository is pinned by the commit itself.
            if action.starts_with("./") {
                continue;
            }
            let (reference, comment) = action.split_once(" # ").unwrap_or((action, ""));
            let commit = reference.rsplit_once('@').map_or("", |(_, commit)| commit);
            assert!(
                commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
                "{workflow}: `{line}` is not pinned to a full commit SHA"
            );
            assert!(
                !comment.trim().is_empty(),
                "{workflow}: `{line}` does not say which version the commit is"
            );
        }
    }
}

#[test]
fn the_binaries_workflow_can_write_nothing() {
    let text = read(BINARIES);
    assert!(
        text.contains("\npermissions:\n  contents: read\n"),
        "{BINARIES} grants `contents: read` at the top level"
    );
    for line in code_lines(&text) {
        let value = line.split_once(':').map_or("", |(_, value)| value.trim());
        assert!(
            value != "write" && value != "write-all",
            "{BINARIES} grants a write permission: `{line}`"
        );
    }
    for forbidden in [
        "secrets.",
        "gh release",
        "git tag",
        "git push",
        "cargo publish",
        "id-token",
    ] {
        assert!(
            !text.contains(forbidden),
            "{BINARIES} holds `{forbidden}`, and it may not publish or use a secret"
        );
    }
    assert_eq!(
        keys_under(&text, "on"),
        ["workflow_dispatch", "workflow_call"],
        "{BINARIES} starts by hand or from another workflow only"
    );

    // The release workflow calls it with read permission only.
    let release = read(RELEASE);
    let caller = job(&release, "binaries");
    assert!(caller.contains(&"    uses: ./.github/workflows/binaries.yml"));
    assert!(caller.contains(&"    needs: checks"));
    let permissions = caller
        .iter()
        .position(|line| *line == "    permissions:")
        .expect("the binaries job of the release workflow sets its permissions");
    let granted: Vec<&str> = caller[permissions + 1..]
        .iter()
        .take_while(|line| line.starts_with("      "))
        .map(|line| line.trim())
        .collect();
    assert_eq!(granted, ["contents: read"]);
}

#[test]
fn the_release_workflow_starts_by_hand_only() {
    let text = read(RELEASE);
    assert_eq!(
        keys_under(&text, "on"),
        ["workflow_dispatch"],
        "{RELEASE} has a trigger other than workflow_dispatch"
    );
    let jobs = keys_under(&text, "jobs");
    for name in ["checks", "binaries", "dry-run", "publish"] {
        assert!(
            jobs.iter().any(|job| job == name),
            "{RELEASE} has no job `{name}`"
        );
    }
    // A failed binary build must not hold back the crates, and the crates
    // must not wait for something that publishes nothing.
    for name in ["dry-run", "publish"] {
        for line in job(&text, name) {
            if line.trim_start().starts_with("needs:") {
                assert!(
                    !line.contains("binaries"),
                    "the {name} job of {RELEASE} needs the binaries: `{line}`"
                );
            }
        }
    }
}

#[test]
fn the_binaries_workflow_builds_every_documented_target() {
    let workflow = read(BINARIES);
    let lines = code_lines(&workflow);
    let mut built = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(target) = line.trim_start().strip_prefix("- target: ") {
            let runner = lines
                .get(i + 1)
                .and_then(|next| next.trim_start().strip_prefix("runner: "))
                .unwrap_or_else(|| panic!("{BINARIES}: the target {target} has no runner"));
            built.push((target.to_owned(), runner.to_owned()));
        }
    }

    let releasing = read("RELEASING.md");
    let section = releasing
        .split("\n## Binaries\n")
        .nth(1)
        .expect("RELEASING.md has a section `## Binaries`");
    let section = section.split("\n## ").next().unwrap_or(section);
    let mut documented = Vec::new();
    for line in section.lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if let [target, runner] = cells.as_slice() {
            let target = target.trim_matches('`');
            let runner = runner.trim_matches('`');
            if target.contains('-') && !target.starts_with("--") {
                documented.push((target.to_owned(), runner.to_owned()));
            }
        }
    }

    assert_eq!(built.len(), 6, "{BINARIES} builds six targets: {built:?}");
    assert_eq!(
        built, documented,
        "the targets and runners of {BINARIES} differ from the table in RELEASING.md, \"Binaries\""
    );
}
