use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{stderr, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use clap::Parser;
use indoc::{eprintdoc, indoc};
use nix_index::{
    database,
    files::{FileTreeEntry, FileType},
};
use regex::bytes::Regex;
use thiserror::Error;

#[derive(Error, Debug)]
enum Error {
    #[error("reading from database '{database}' failed: {source}.\n{extra}", extra = indoc!("
        This may be caused by a corrupt or missing database, try (re)running `nix-index` to generate the database.
        If the error persists please file a bug report at https://github.com/nix-community/nix-index.
    "))]
    ReadDatabase {
        database: PathBuf,
        #[source]
        source: database::Error,
    },
    #[error("constructing the regular expression from the pattern '{pattern}' failed: {source}")]
    Grep {
        pattern: String,
        #[source]
        source: regex::Error,
    },
    #[error("searching the database at '{database}' failed: {source}")]
    SearchDatabase {
        database: PathBuf,
        #[source]
        source: database::Error,
    },
}

fn cache_dir() -> &'static OsStr {
    let base = xdg::BaseDirectories::with_prefix("nix-index");
    let cache_dir = Box::new(base.get_cache_home().unwrap());
    let cache_dir = Box::leak(cache_dir);
    cache_dir.as_os_str()
}

#[derive(Parser)]
struct Cli {
    /// The binary to locate
    binary: String,

    /// Directory where the index is stored
    #[arg(long = "db", default_value_os = cache_dir(), env = "NIX_INDEX_DATABASE")]
    database: PathBuf,
}

fn run(args: &Cli) -> Result<HashSet<String>, Error> {
    let index_file = args.database.join("files");
    let db = database::Reader::open(&index_file).map_err(|e| Error::ReadDatabase {
        database: index_file.clone(),
        source: e,
    })?;

    let pattern = format!("^/bin/{}$", regex::escape(&args.binary));
    let regex = Regex::new(&pattern).map_err(|e| Error::Grep {
        pattern: pattern.clone(),
        source: e,
    })?;

    db.query(&regex)
        .run()
        .map_err(|e| Error::SearchDatabase {
            database: index_file.clone(),
            source: e,
        })?
        .filter(|v| {
            v.as_ref().map_or(true, |v| {
                let (ref store_path, FileTreeEntry { path: _, ref node }) = v;

                let conditions = [
                    store_path.origin().toplevel,
                    matches!(node.get_type(), FileType::Regular { executable: true }),
                ];

                conditions.iter().all(|c| *c)
            })
        })
        .map(|v| {
            v.map(|v| {
                let (store_path, _) = v;
                format!(
                    "{}.{}",
                    store_path.origin().attr,
                    store_path.origin().output
                )
            })
            .map_err(|e| Error::ReadDatabase {
                database: index_file.clone(),
                source: e,
            })
        })
        .collect()
}

fn main() {
    let args = Cli::parse();

    match run(&args) {
        Ok(attrs) => {
            eprintdoc!(
                "
                The program '{}' is currently not installed.
                You can install it with one of the following packages:
                ",
                args.binary
            );

            // pipe to column because i'm lazy
            let mut child = Command::new("column")
                .arg("-x")
                .stdin(Stdio::piped())
                .stdout(stderr())
                .stderr(stderr())
                .spawn()
                .expect("failed to execute child");
            let mut stdin = child.stdin.take().expect("handle present");

            for attr in attrs.into_iter() {
                // two spaces for padding
                writeln!(stdin, "  {}", attr).expect("failed to write to child's stdin");
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
