#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use fingerprint_kit::{
    MatchOnChipDevice, Result, TemplateRecord, WorkerDevice, create_synthetic_bundle,
    enroll_bundles, enroll_device_template, inspect_bundle, run_demo, verify_bundle,
    verify_device_template,
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
    /// Enroll a finger on a match-on-chip device driven by a driver worker.
    EnrollDevice {
        /// New template JSON path. Existing files are never overwritten.
        #[arg(long)]
        out: PathBuf,
        /// The driver worker to spawn.
        #[arg(long)]
        driver: String,
        /// Arguments passed to the driver worker.
        #[arg(last = true)]
        driver_args: Vec<String>,
    },
    /// Verify a live finger on a match-on-chip device against a stored template.
    VerifyDevice {
        /// Versioned template JSON path.
        #[arg(long)]
        template: PathBuf,
        /// The driver worker to spawn.
        #[arg(long)]
        driver: String,
        /// Arguments passed to the driver worker.
        #[arg(last = true)]
        driver_args: Vec<String>,
    },
    /// Run the complete temporary hardware-free vertical slice.
    Demo,
}

/// Spawn a driver worker and report what it bound to, so the operator knows what they are about
/// to present a finger to.
fn spawn_driver(driver: &str, driver_args: &[String]) -> Result<WorkerDevice> {
    let arguments: Vec<&str> = driver_args.iter().map(String::as_str).collect();
    let device = WorkerDevice::spawn(driver, &arguments)?;
    let info = device.device();
    println!(
        "device: {} via {} ({} enroll stages)",
        info.device_id, info.capture_profile_id, info.enroll_stages
    );
    Ok(device)
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
            let samples = match &template {
                TemplateRecord::HostImage(template) => template.samples.len(),
                // Not reachable from this command, which enrolls from capture bundles. Matched
                // rather than unwrapped so a future arm is a compile error, not a panic.
                TemplateRecord::MatchOnChip(_) => 0,
            };
            println!(
                "enrolled {samples} samples with policy {}",
                template.policy()
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
        Command::EnrollDevice {
            out,
            driver,
            driver_args,
        } => {
            let mut device = spawn_driver(&driver, &driver_args)?;
            let stages = device.enroll_stages();
            println!("present the same finger {stages} times when the sensor asks");
            let template = enroll_device_template(&mut device, &out, &mut |completed, total| {
                // Deliberately not "n of total remaining": the last stage is often not reported,
                // so counting down to zero would be a promise this cannot keep.
                println!("  stage {completed}/{total} accepted");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            })?;
            println!(
                "enrolled on {} with policy {}",
                template.driver_id, template.policy
            );
            Ok(0)
        }
        Command::VerifyDevice {
            template,
            driver,
            driver_args,
        } => {
            let mut device = spawn_driver(&driver, &driver_args)?;
            // Not "present the enrolled finger": this is also how you check that a stranger's
            // finger is refused, and telling the operator which one to use would beg the question.
            println!("present a finger");
            let verdict = verify_device_template(&mut device, &template)?;
            println!("{}", if verdict.matched { "MATCH" } else { "NO MATCH" });
            Ok(u8::from(!verdict.matched))
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
