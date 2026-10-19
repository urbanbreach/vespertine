//! CMP-026 A2 (line-numbered validation), A9 (limits), program convention.

use vespertine_components::shader::program::{
    MAX_SOURCE_BYTES, ProgramError, assemble, validate_program,
};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

const GOOD: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float pulse = 0.5 + 0.5 * sin(iTime);
    fragColor = vec4(uv, pulse, 1.0) * texture(iChannel0, uv).a + vec4(iMouse.xy / iResolution.xy, 0.0, 0.0);
}";

#[test]
fn a_valid_program_compiles() {
    validate_program(GOOD, &[]).unwrap();
}

#[test]
fn named_inputs_are_exposed_under_their_identifiers() {
    let program = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(gain, bias, speed.x, 1.0);
}";
    let error = validate_program(program, &names(&["gain", "bias"])).unwrap_err();
    // `speed.x` is not an input here: reported on the author's line 2.
    assert_eq!(error.line, Some(2), "{error}");
    let program = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(gain, bias, speed, 1.0);
}";
    validate_program(program, &names(&["gain", "bias", "speed"])).unwrap();
}

#[test]
fn errors_carry_the_line_in_the_authors_text() {
    let program = "// line 1
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float a = 1.0;
    fragColor = vec4(undefined_name, a, 0.0, 1.0);
}";
    let error = validate_program(program, &[]).unwrap_err();
    assert_eq!(error.line, Some(4), "{error}");
    assert!(!error.message.is_empty());
    // A type error is found by the validator, still on the author's line.
    let program = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(1.0);
    int x = 1.5;
}";
    let error = validate_program(program, &[]).unwrap_err();
    assert_eq!(error.line, Some(3), "{error}");
}

#[test]
fn a_missing_entry_function_is_reported_without_a_line() {
    let error = validate_program("void other() {}", &[]).unwrap_err();
    assert_eq!(error.line, None);
    assert!(error.message.contains("mainImage"));
}

#[test]
fn precision_statements_are_removed_and_keep_line_numbers() {
    let program = "precision highp float;
  precision mediump int;
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(oops, 0.0, 0.0, 1.0);
}";
    let assembled = assemble(program, &[]).unwrap();
    assert!(!assembled.source.contains("precision"));
    let error = validate_program(program, &[]).unwrap_err();
    assert_eq!(error.line, Some(4), "{error}");
}

#[test]
fn a9_limits_are_enforced() {
    let too_long = format!("{GOOD}\n{}", "/".repeat(MAX_SOURCE_BYTES));
    assert!(too_long.len() > MAX_SOURCE_BYTES);
    let error = validate_program(&too_long, &[]).unwrap_err();
    assert!(error.message.contains("exceeds"), "{error}");
    // 65,536 bytes exactly is accepted.
    let padding = MAX_SOURCE_BYTES - GOOD.len() - 1;
    let exact = format!("{GOOD}\n{}", "/".repeat(padding));
    assert_eq!(exact.len(), MAX_SOURCE_BYTES);
    validate_program(&exact, &[]).unwrap();
    let seventeen: Vec<String> = (0..17).map(|i| format!("v{i}")).collect();
    let error = validate_program(GOOD, &seventeen).unwrap_err();
    assert!(error.message.contains("17"), "{error}");
    let sixteen: Vec<String> = (0..16).map(|i| format!("v{i}")).collect();
    validate_program(GOOD, &sixteen).unwrap();
}

#[test]
fn a9_input_names_must_be_identifiers_that_do_not_collide() {
    for bad in [
        "iTime",
        "iResolution",
        "iChannel2",
        "mainImage",
        "main",
        "vsp_x",
        "gl_Thing",
        "1abc",
        "a-b",
        "",
        "has space",
    ] {
        let error = validate_program(GOOD, &names(&[bad])).unwrap_err();
        assert_eq!(error.line, None, "{bad}");
    }
    assert!(validate_program(GOOD, &names(&["twice", "twice"])).is_err());
    validate_program(GOOD, &names(&["_ok", "Value2", "iTimes"])).unwrap();
    let _: Option<ProgramError> = None;
}
