#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fingerprint-kit"))
        .args(arguments)
        .output()
        .unwrap()
}

fn path_text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn synthetic_inspect_enroll_and_verify_round_trip() {
    let temp = tempdir().unwrap();
    let captures: Vec<_> = (0..3)
        .map(|index| temp.path().join(format!("capture-{index}")))
        .collect();
    for (index, capture) in captures.iter().enumerate() {
        let output = run(&[
            "synthetic",
            "--identity",
            "41",
            "--impression",
            &index.to_string(),
            "--out",
            path_text(capture),
        ]);
        assert!(output.status.success(), "{output:?}");
    }

    let inspect = run(&["inspect", path_text(&captures[0])]);
    assert!(inspect.status.success(), "{inspect:?}");
    let inspect_stdout = String::from_utf8(inspect.stdout).unwrap();
    assert!(inspect_stdout.contains("image: valid Gray8 200x240 @ 500x500 ppi"));
    assert!(inspect_stdout.contains("dynamic-range:"));
    assert!(inspect_stdout.contains("minutiae:"));
    assert!(inspect_stdout.contains("mean-quality:"));

    let template = temp.path().join("template.json");
    let enroll = run(&[
        "enroll",
        "--out",
        path_text(&template),
        path_text(&captures[0]),
        path_text(&captures[1]),
        path_text(&captures[2]),
    ]);
    assert!(enroll.status.success(), "{enroll:?}");

    let genuine = temp.path().join("genuine");
    assert!(
        run(&[
            "synthetic",
            "--identity",
            "41",
            "--impression",
            "90",
            "--out",
            path_text(&genuine),
        ])
        .status
        .success()
    );
    let genuine_verify = run(&[
        "verify",
        "--template",
        path_text(&template),
        path_text(&genuine),
    ]);
    assert_eq!(genuine_verify.status.code(), Some(0), "{genuine_verify:?}");
    assert!(
        String::from_utf8(genuine_verify.stdout)
            .unwrap()
            .contains("MATCH")
    );

    let impostor = temp.path().join("impostor");
    assert!(
        run(&[
            "synthetic",
            "--identity",
            "99",
            "--impression",
            "0",
            "--out",
            path_text(&impostor),
        ])
        .status
        .success()
    );
    let impostor_verify = run(&[
        "verify",
        "--template",
        path_text(&template),
        path_text(&impostor),
    ]);
    assert_eq!(
        impostor_verify.status.code(),
        Some(1),
        "{impostor_verify:?}"
    );
    assert!(
        String::from_utf8(impostor_verify.stdout)
            .unwrap()
            .contains("NO MATCH")
    );
}

#[test]
fn processing_errors_exit_two_and_outputs_are_not_overwritten() {
    let temp = tempdir().unwrap();
    let capture = temp.path().join("capture");
    assert!(
        run(&[
            "synthetic",
            "--identity",
            "1",
            "--impression",
            "1",
            "--out",
            path_text(&capture),
        ])
        .status
        .success()
    );
    let second = run(&[
        "synthetic",
        "--identity",
        "1",
        "--impression",
        "1",
        "--out",
        path_text(&capture),
    ]);
    assert_eq!(second.status.code(), Some(2));

    let malformed_verify = run(&["verify", "--template", "missing-template.json"]);
    assert_eq!(malformed_verify.status.code(), Some(2));
    assert_eq!(run(&[]).status.code(), Some(2));
}

#[test]
fn demo_completes_the_temporary_vertical_slice() {
    let output = run(&["demo"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("MATCH"));
    assert!(stdout.contains("NO MATCH"));
}
