use std::process::Command;

const EXAMPLES: &[&str] = &[
    "hello_database",
    "geometry_render",
    "bindless_render",
    "image_blit",
    "model_render",
];

#[test]
fn run_examples() {
    for example in EXAMPLES {
        let status = Command::new("cargo")
            .args(["run", "--example", example, "--quiet"])
            .status()
            .expect("failed to launch cargo");

        assert!(
            status.success(),
            "example `{example}` did not exit successfully"
        );
    }
}
