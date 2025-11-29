use std::path::Path;

use dashi::{BindGroupLayout, BindGroupLayoutInfo, Context, GPUError, Handle, cfg};

use crate::utils::NorenError;

/// Owns the YAML-authored configuration for a [`BindGroupLayout`].
///
/// The configuration can be parsed from a YAML document and later borrowed to
/// produce the [`BindGroupLayoutInfo`] required by dashi when creating the
/// runtime layout object.
#[derive(Clone, Debug)]
pub struct BindGroupLayoutTemplate {
    cfg: cfg::BindGroupLayoutCfg,
}

impl BindGroupLayoutTemplate {
    /// Parse a bind group layout template from a YAML string.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, NorenError> {
        let cfg = cfg::BindGroupLayoutCfg::from_yaml(yaml)?;
        Ok(Self { cfg })
    }

    /// Load a bind group layout template from a YAML file on disk.
    pub fn from_yaml_file(path: impl AsRef<Path>) -> Result<Self, NorenError> {
        let yaml = std::fs::read_to_string(path)?;
        Self::from_yaml_str(&yaml)
    }

    /// Name for the bind group layout.
    pub fn debug_name(&self) -> &str {
        &self.cfg.debug_name
    }

    /// Build a borrowed view that exposes [`BindGroupLayoutInfo`] data.
    pub fn borrow(&self) -> cfg::BindGroupLayoutBorrowed<'_> {
        self.cfg.borrow()
    }

    /// Convenience helper that constructs the runtime [`BindGroupLayout`]
    /// directly from the stored configuration.
    pub fn create_layout(&self, ctx: &mut Context) -> Result<Handle<BindGroupLayout>, GPUError> {
        let borrowed = self.borrow();
        let info: BindGroupLayoutInfo<'_> = borrowed.info();
        ctx.make_bind_group_layout(&info)
    }
}

/// Parse a list of bind group layout templates from a YAML string.
pub fn parse_bind_group_layout_templates(
    yaml: &str,
) -> Result<Vec<BindGroupLayoutTemplate>, NorenError> {
    let cfgs = cfg::BindGroupLayoutCfg::vec_from_yaml(yaml)?;
    Ok(cfgs
        .into_iter()
        .map(|cfg| BindGroupLayoutTemplate { cfg })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashi::{BindGroupVariableType, ShaderType};

    #[test]
    fn parse_single_template_from_yaml() {
        let yaml = r#"
---
debug_name: "test_layout"
shaders:
  - stage: Vertex
    variables:
      - var_type: Uniform
        binding: 0
        count: 1
  - stage: Fragment
    variables:
      - var_type: SampledImage
        binding: 1
        count: 4
"#;

        let template =
            BindGroupLayoutTemplate::from_yaml_str(yaml).expect("parse bind group layout template");
        assert_eq!(template.debug_name(), "test_layout");

        let borrowed = template.borrow();
        let info = borrowed.info();

        assert_eq!(info.debug_name, "test_layout");
        assert_eq!(info.shaders.len(), 2);
        assert_eq!(info.shaders[0].shader_type, ShaderType::Vertex);
        assert_eq!(info.shaders[0].variables.len(), 1);
        assert_eq!(
            info.shaders[0].variables[0].var_type,
            BindGroupVariableType::Uniform
        );
        assert_eq!(info.shaders[1].shader_type, ShaderType::Fragment);
        assert_eq!(
            info.shaders[1].variables[0].var_type,
            BindGroupVariableType::SampledImage
        );
        assert_eq!(info.shaders[1].variables[0].count, 4);
    }

    #[test]
    fn parse_multiple_templates_from_yaml() {
        let yaml = r#"
- debug_name: "layout_a"
  shaders:
    - stage: Vertex
      variables:
        - var_type: Uniform
          binding: 0
          count: 1
- debug_name: "layout_b"
  shaders:
    - stage: Fragment
      variables:
        - var_type: Storage
          binding: 1
          count: 1
"#;

        let templates =
            parse_bind_group_layout_templates(yaml).expect("parse bind group layout templates");
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].debug_name(), "layout_a");
        assert_eq!(templates[1].debug_name(), "layout_b");

        let borrowed_a = templates[0].borrow();
        let info_a = borrowed_a.info();
        assert_eq!(info_a.shaders.len(), 1);
        assert_eq!(info_a.shaders[0].shader_type, ShaderType::Vertex);
        assert_eq!(info_a.shaders[0].variables[0].binding, 0);

        let borrowed_b = templates[1].borrow();
        let info_b = borrowed_b.info();
        assert_eq!(info_b.shaders[0].shader_type, ShaderType::Fragment);
        assert_eq!(
            info_b.shaders[0].variables[0].var_type,
            BindGroupVariableType::Storage
        );
    }
}
