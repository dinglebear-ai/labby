//! Stable display metadata for Labby-owned descriptors only.

use std::sync::LazyLock;

use base64::Engine as _;
use rmcp::model::{Icon, Prompt, Resource, ResourceTemplate, Tool};

fn icon() -> Icon {
    static ICON: LazyLock<Icon> = LazyLock::new(|| {
        let bytes = include_bytes!("../../../../apps/web/public/icon-dark-32x32.png");
        Icon::new(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
        .with_mime_type("image/png")
        .with_sizes(vec!["32x32".to_owned()])
    });
    ICON.clone()
}

fn title(name: &str) -> String {
    name.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| match word {
            "api" => "API".to_owned(),
            "mcp" => "MCP".to_owned(),
            "ui" => "UI".to_owned(),
            "codemode" => "Code Mode".to_owned(),
            _ => {
                let mut characters = word.chars();
                characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + characters.as_str()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Add native display metadata without replacing an authored title or icon.
pub(crate) fn native_tool(mut tool: Tool) -> Tool {
    tool.title.get_or_insert_with(|| title(&tool.name));
    tool.icons.get_or_insert_with(|| vec![icon()]);
    tool
}

/// Add native display metadata to a locally authored prompt.
pub(crate) fn native_prompt(mut prompt: Prompt) -> Prompt {
    prompt.title.get_or_insert_with(|| title(&prompt.name));
    prompt.icons.get_or_insert_with(|| vec![icon()]);
    prompt
}

/// Add native display metadata to a locally authored resource.
pub(crate) fn native_resource(mut resource: Resource) -> Resource {
    resource.title.get_or_insert_with(|| title(&resource.name));
    resource.icons.get_or_insert_with(|| vec![icon()]);
    resource
}

/// Add native display metadata to a locally authored resource template.
pub(crate) fn native_resource_template(mut template: ResourceTemplate) -> ResourceTemplate {
    template.title.get_or_insert_with(|| title(&template.name));
    template.icons.get_or_insert_with(|| vec![icon()]);
    template
}
