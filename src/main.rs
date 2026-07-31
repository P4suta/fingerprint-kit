#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use fingerprint_kit::{
    Result, create_synthetic_bundle, enroll_bundles, inspect_bundle, run_demo, verify_bundle,
};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "fingerprint-kit",
    version,
    about = "Hardware-free experimental fingerprint pipeline"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a deterministic synthetic capture bundle.
    Synthetic {
        /// Synthetic finger identity seed.
        #[arg(long)]
        identity: u64,
        /// Recapture/impression seed for the identity.
        #[arg(long)]
        impression: u64,
        /// New output directory. Existing paths are never overwritten.
        #[arg(long)]
        out: PathBuf,
    },
    /// Validate and summarize a replay capture bundle.
    Inspect {
        /// Capture bundle directory.
        directory: PathBuf,
    },
    /// Enroll exactly three capture bundles into a new template.
    Enroll {
        /// New template JSON path. Existing files are never overwritten.
        #[arg(long)]
        out: PathBuf,
        /// Exactly three capture bundle directories.
        #[arg(required = true, num_args = 3)]
        captures: Vec<PathBuf>,
    },
    /// Verify one capture against a template.
    Verify {
        /// Versioned template JSON path.
        #[arg(long)]
        template: PathBuf,
        /// Probe capture bundle directory.
        capture: PathBuf,
    },
    /// Run the complete temporary hardware-free vertical slice.
    Demo,
}

fn main() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::Synthetic {
            identity,
            impression,
            out,
        } => {
            create_synthetic_bundle(identity, impression, &out)?;
            println!("synthetic capture created");
            Ok(0)
        }
        Command::Inspect { directory } => {
            let inspection = inspect_bundle(&directory)?;
            println!(
                "image: valid Gray8 {}x{} @ {}x{} ppi",
                inspection.width,
                inspection.height,
                inspection.x_resolution_ppi,
                inspection.y_resolution_ppi
            );
            println!("dynamic-range: {}", inspection.dynamic_range);
            println!("minutiae: {}", inspection.minutiae_count);
            println!("mean-quality: {:.1}", inspection.mean_quality);
            Ok(0)
        }
        Command::Enroll { out, captures } => {
            let template = enroll_bundles(&captures, &out)?;
            println!(
                "enrolled {} samples with policy {}",
                template.samples.len(),
                template.policy
            );
            Ok(0)
        }
        Command::Verify { template, capture } => {
            let result = verify_bundle(&template, &capture)?;
            let verdict = if result.matched { "MATCH" } else { "NO MATCH" };
            println!(
                "score: {} threshold: {} probe-quality: {} {verdict}",
                result.score, result.threshold, result.probe_quality
            );
            Ok(u8::from(!result.matched))
        }
        Command::Demo => {
            let (genuine, impostor) = run_demo()?;
            println!(
                "genuine: score {} MATCH; impostor: score {} NO MATCH",
                genuine.score, impostor.score
            );
            Ok(0)
        }
    }
}
