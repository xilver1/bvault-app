//! `pdbdiff` — page-aware diff of two export.pdb files (golden-file harness).
//!
//! Usage:
//!     cargo run -p bvault-core --bin pdbdiff -- <golden.pdb> <mine.pdb>
//!
//! Exit code 0 if identical, 1 if differences were found, 2 on I/O error.

use std::process::ExitCode;

use bvault_core::diff_pdb;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <golden.pdb> <mine.pdb>", args[0]);
        return ExitCode::from(2);
    }

    let golden = match std::fs::read(&args[1]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error reading {}: {}", args[1], e);
            return ExitCode::from(2);
        }
    };
    let mine = match std::fs::read(&args[2]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error reading {}: {}", args[2], e);
            return ExitCode::from(2);
        }
    };

    let d = diff_pdb(&golden, &mine);
    print!("{}", d.report());

    if d.is_identical() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}