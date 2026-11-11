//! Palettes for the Agent OS surface. Every role in the transcript owns a
//! hue: user input, the assistant, each narration kind, every tool category
//! (fs / shell / memory / skills / web / workforce / meta / mcp / plugin),
//! approvals, refusals and status states. Three shipped themes:
//!
//! * `midnight` — GitHub-dark inspired, the default
//! * `nordic`   — Nord palette, cool and calm
//! * `ember`    — Gruvbox-inspired, warm and high-contrast
//!
//! All colors are truecolor RGB; terminals without truecolor support see
//! approximate ANSI fallbacks from ratatui.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub name: &'static str,
    // conversation
    pub user: Color,
    pub assistant: Color,
    // narration kinds
    pub ack: Color,
    pub progress: Color,
    pub finding: Color,
    pub decision: Color,
    pub completion: Color,
    // tool categories
    pub tool_fs: Color,
    pub tool_shell: Color,
    pub tool_mem: Color,
    pub tool_skills: Color,
    pub tool_web: Color,
    pub tool_agent: Color,
    pub tool_meta: Color,
    pub tool_mcp: Color,
    pub tool_plugin: Color,
    // outcomes
    pub ok: Color,
    pub fail: Color,
    pub refused: Color,
    pub approval: Color,
    // chrome
    pub accent: Color,
    pub info: Color,
    pub dim: Color,
    pub border: Color,
    pub border_focus: Color,
    pub status_ready: Color,
    pub status_work: Color,
    pub status_stream: Color,
    pub status_approval: Color,
}

impl Palette {
    pub fn fg(self, c: Color) -> Style {
        Style::default().fg(c)
    }

    pub fn bold(self, c: Color) -> Style {
        Style::default().fg(c).add_modifier(Modifier::BOLD)
    }

    pub fn italic(self, c: Color) -> Style {
        Style::default().fg(c).add_modifier(Modifier::ITALIC)
    }

    pub fn dim(self) -> Style {
        Style::default().fg(self.dim)
    }

    /// Style for a tool receipt line, keyed by tool-name prefix. Unknown
    /// prefixes fall back to the accent color so nothing renders invisible.
    pub fn tool_color(self, tool: &str) -> Color {
        let c = if tool.starts_with("fs_") {
            self.tool_fs
        } else if tool.starts_with("shell_") {
            self.tool_shell
        } else if tool.starts_with("mem_") {
            self.tool_mem
        } else if tool.starts_with("skills_") {
            self.tool_skills
        } else if tool.starts_with("web_") {
            self.tool_web
        } else if tool.starts_with("agent_") {
            self.tool_agent
        } else if tool.starts_with("capability_") {
            self.tool_meta
        } else if tool.starts_with("mcp_") {
            self.tool_mcp
        } else if tool.starts_with("plugin_") {
            self.tool_plugin
        } else {
            self.accent
        };
        c
    }
}

pub const MIDNIGHT: Palette = Palette {
    name: "midnight",
    user: Color::Rgb(126, 231, 135),      // #7ee787
    assistant: Color::Rgb(230, 237, 243), // #e6edf3
    ack: Color::Rgb(121, 192, 255),       // #79c0ff
    progress: Color::Rgb(210, 168, 255),  // #d2a8ff
    finding: Color::Rgb(126, 231, 167),   // #7ee7a7
    decision: Color::Rgb(255, 166, 87),   // #ffa657
    completion: Color::Rgb(86, 211, 100), // #56d364
    tool_fs: Color::Rgb(121, 184, 255),   // #79b8ff
    tool_shell: Color::Rgb(255, 166, 87), // #ffa657
    tool_mem: Color::Rgb(188, 140, 255),  // #bc8cff
    tool_skills: Color::Rgb(57, 197, 207),// #39c5cf
    tool_web: Color::Rgb(86, 212, 221),   // #56d4dd
    tool_agent: Color::Rgb(145, 230, 177),// #91e6b1
    tool_meta: Color::Rgb(139, 148, 158), // #8b949e
    tool_mcp: Color::Rgb(242, 204, 96),   // #f2cc60
    tool_plugin: Color::Rgb(255, 155, 206),// #ff9bce
    ok: Color::Rgb(86, 211, 100),         // #56d364
    fail: Color::Rgb(255, 123, 114),      // #ff7b72
    refused: Color::Rgb(227, 179, 65),    // #e3b341
    approval: Color::Rgb(210, 168, 255),  // #d2a8ff
    accent: Color::Rgb(88, 166, 255),     // #58a6ff
    info: Color::Rgb(139, 148, 158),      // #8b949e
    dim: Color::Rgb(110, 118, 129),       // #6e7681
    border: Color::Rgb(48, 54, 61),       // #30363d
    border_focus: Color::Rgb(88, 166, 255),
    status_ready: Color::Rgb(86, 211, 100),
    status_work: Color::Rgb(88, 166, 255),
    status_stream: Color::Rgb(121, 192, 255),
    status_approval: Color::Rgb(210, 168, 255),
};

pub const NORDIC: Palette = Palette {
    name: "nordic",
    user: Color::Rgb(163, 190, 140),      // nord14 aurora green
    assistant: Color::Rgb(236, 239, 244), // nord6 snow
    ack: Color::Rgb(136, 192, 208),       // nord8 frost
    progress: Color::Rgb(129, 161, 193),  // nord10 frost deep
    finding: Color::Rgb(163, 190, 140),   // nord14
    decision: Color::Rgb(235, 203, 139),  // nord13 yellow
    completion: Color::Rgb(163, 190, 140),
    tool_fs: Color::Rgb(129, 161, 193),   // nord10
    tool_shell: Color::Rgb(208, 135, 112),// nord12 orange
    tool_mem: Color::Rgb(180, 142, 173),  // nord15 violet
    tool_skills: Color::Rgb(136, 192, 208),// nord8
    tool_web: Color::Rgb(151, 187, 173),  // nord7
    tool_agent: Color::Rgb(163, 190, 140),// nord14
    tool_meta: Color::Rgb(110, 118, 129),
    tool_mcp: Color::Rgb(235, 203, 139),  // nord13
    tool_plugin: Color::Rgb(191, 97, 106),// nord11 red (pink-ish here)
    ok: Color::Rgb(163, 190, 140),
    fail: Color::Rgb(191, 97, 106),       // nord11
    refused: Color::Rgb(235, 203, 139),   // nord13
    approval: Color::Rgb(180, 142, 173),  // nord15
    accent: Color::Rgb(136, 192, 208),    // nord8
    info: Color::Rgb(94, 105, 121),       // nord3-ish
    dim: Color::Rgb(76, 86, 106),         // nord3
    border: Color::Rgb(59, 66, 82),       // nord1
    border_focus: Color::Rgb(136, 192, 208),
    status_ready: Color::Rgb(163, 190, 140),
    status_work: Color::Rgb(136, 192, 208),
    status_stream: Color::Rgb(129, 161, 193),
    status_approval: Color::Rgb(180, 142, 173),
};

pub const EMBER: Palette = Palette {
    name: "ember",
    user: Color::Rgb(184, 187, 38),       // gruvbox green
    assistant: Color::Rgb(235, 219, 178), // fg
    ack: Color::Rgb(131, 165, 152),       // aqua
    progress: Color::Rgb(177, 98, 134),   // purple
    finding: Color::Rgb(142, 192, 124),   // neutral green
    decision: Color::Rgb(254, 128, 25),   // orange
    completion: Color::Rgb(184, 187, 38),
    tool_fs: Color::Rgb(131, 165, 152),   // aqua
    tool_shell: Color::Rgb(254, 128, 25), // orange
    tool_mem: Color::Rgb(211, 134, 155),  // purple
    tool_skills: Color::Rgb(142, 192, 124),// green
    tool_web: Color::Rgb(135, 175, 166),  // aqua dim
    tool_agent: Color::Rgb(184, 187, 38),
    tool_meta: Color::Rgb(146, 131, 116), // gray
    tool_mcp: Color::Rgb(250, 189, 47),   // yellow
    tool_plugin: Color::Rgb(251, 73, 52), // red (warm pink)
    ok: Color::Rgb(184, 187, 38),
    fail: Color::Rgb(251, 73, 52),        // red
    refused: Color::Rgb(250, 189, 47),    // yellow
    approval: Color::Rgb(211, 134, 155),  // purple
    accent: Color::Rgb(250, 189, 47),     // yellow
    info: Color::Rgb(146, 131, 116),
    dim: Color::Rgb(124, 111, 100),
    border: Color::Rgb(80, 73, 69),
    border_focus: Color::Rgb(250, 189, 47),
    status_ready: Color::Rgb(184, 187, 38),
    status_work: Color::Rgb(250, 189, 47),
    status_stream: Color::Rgb(131, 165, 152),
    status_approval: Color::Rgb(211, 134, 155),
};

pub const THEMES: [&str; 3] = ["midnight", "nordic", "ember"];

/// Resolve a theme by name; unknown names fall back to `midnight`.
pub fn by_name(name: &str) -> Palette {
    match name {
        "nordic" => NORDIC,
        "ember" => EMBER,
        _ => MIDNIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_resolve_and_unknown_falls_back() {
        assert_eq!(by_name("nordic").name, "nordic");
        assert_eq!(by_name("ember").name, "ember");
        assert_eq!(by_name("midnight").name, "midnight");
        assert_eq!(by_name("nope").name, "midnight");
    }

    #[test]
    fn tool_categories_own_distinct_hues() {
        // The whole point: a receipt is recognizable by color alone.
        let p = MIDNIGHT;
        let hues = [
            p.tool_color("fs_read"),
            p.tool_color("shell_run"),
            p.tool_color("mem_recall"),
            p.tool_color("skills_load"),
            p.tool_color("web_fetch"),
            p.tool_color("mcp_demo_time"),
            p.tool_color("plugin_greet"),
        ];
        let as_u32 = |c: Color| -> u32 {
            match c {
                Color::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
                _ => 0,
            }
        };
        // fs/shell/mem/mcp/plugin at least must differ pairwise.
        let core = [hues[0], hues[1], hues[2], hues[5], hues[6]];
        for i in 0..core.len() {
            for j in (i + 1)..core.len() {
                assert_ne!(as_u32(core[i]), as_u32(core[j]), "hues {i} and {j} collide");
            }
        }
    }

    #[test]
    fn unknown_tool_prefix_gets_accent_not_invisible() {
        assert_eq!(MIDNIGHT.tool_color("weird_tool"), MIDNIGHT.accent);
    }
}
