//! Page-level animation orchestrations (`Page.animations`).

use serde::{Deserialize, Serialize};

/// One animation bound to a page element via `elementId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    /// Must reference an element id on the same page.
    pub element_id: String,
    pub effect: AnimationEffect,
    /// `"onClick" | "withPrevious" | "afterPrevious"`; defaults to `onClick`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<AnimationTrigger>,
    /// Only used by fly / wipe / peek / float effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<AnimationDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<AnimationEasing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Required for `motion-path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Required for `fill-color` / `color-pulse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Required for `transparency`; target opacity in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
}

/// Animation effects: entrance / emphasis / exit / motion-path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnimationEffect {
    Appear,
    FadeIn,
    FlyIn,
    ZoomIn,
    WipeIn,
    FloatIn,
    PeekIn,
    RiseIn,
    Pulse,
    GrowShrink,
    Spin,
    Teeter,
    FillColor,
    Transparency,
    ColorPulse,
    Disappear,
    FadeOut,
    FlyOut,
    ZoomOut,
    WipeOut,
    FloatOut,
    MotionPath,
}

/// When an animation starts relative to its predecessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnimationTrigger {
    OnClick,
    WithPrevious,
    AfterPrevious,
}

/// Travel / wipe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationEasing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}
