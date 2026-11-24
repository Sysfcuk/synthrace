//! Embedded WGSL sources with a tiny `#include` preprocessor.

const SOURCES: &[(&str, &str)] = &[
    ("common.wgsl", include_str!("../shaders/common.wgsl")),
    ("fullscreen.wgsl", include_str!("../shaders/fullscreen.wgsl")),
    ("sky.wgsl", include_str!("../shaders/sky.wgsl")),
    ("ground.wgsl", include_str!("../shaders/ground.wgsl")),
    ("bloom_down.wgsl", include_str!("../shaders/bloom_down.wgsl")),
    ("bloom_up.wgsl", include_str!("../shaders/bloom_up.wgsl")),
    ("composite.wgsl", include_str!("../shaders/composite.wgsl")),
];

/// Top-level shader modules (each becomes one `wgpu::ShaderModule`).
pub const MODULES: &[&str] = &["sky.wgsl", "ground.wgsl", "bloom_down.wgsl", "bloom_up.wgsl", "composite.wgsl"];

fn raw(name: &str) -> &'static str {
    SOURCES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s).unwrap_or_else(|| panic!("unknown shader source {name}"))
}

/// Expands `#include name.wgsl` lines (one level of nesting is all we use; each file included once).
pub fn compose(name: &str) -> String {
    let mut out = String::new();
    let mut seen: Vec<&str> = Vec::new();
    expand(name, &mut out, &mut seen);
    out
}

fn expand<'a>(name: &'a str, out: &mut String, seen: &mut Vec<&'a str>) {
    if seen.contains(&name) {
        return;
    }
    seen.push(name);
    for line in raw(name).lines() {
        if let Some(inc) = line.trim().strip_prefix("#include ") {
            let inc = inc.trim();
            let key = SOURCES.iter().find(|(n, _)| *n == inc).map(|(n, _)| *n).unwrap_or_else(|| panic!("bad include {inc}"));
            expand(key, out, seen);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_validate(name: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let src = compose(name);
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
        // Validate against the capabilities WebGL2 (GLSL ES 3.00) offers.
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
        (module, info)
    }

    #[test]
    fn all_shaders_validate_and_translate_to_glsl_es_300() {
        for name in MODULES {
            let (module, info) = parse_and_validate(name);
            for ep in &module.entry_points {
                let options = naga::back::glsl::Options {
                    version: naga::back::glsl::Version::Embedded { version: 300, is_webgl: true },
                    ..Default::default()
                };
                let pipeline_options =
                    naga::back::glsl::PipelineOptions { shader_stage: ep.stage, entry_point: ep.name.clone(), multiview: None };
                let mut out = String::new();
                let mut writer = naga::back::glsl::Writer::new(
                    &mut out,
                    &module,
                    &info,
                    &options,
                    &pipeline_options,
                    naga::proc::BoundsCheckPolicies::default(),
                )
                .unwrap_or_else(|e| panic!("{name}:{}: {e:?}", ep.name));
                writer.write().unwrap_or_else(|e| panic!("{name}:{}: {e:?}", ep.name));
                assert!(out.contains("#version 300 es"), "{name}:{}", ep.name);
            }
        }
    }
}
