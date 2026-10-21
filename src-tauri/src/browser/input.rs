//! 面板直接输入：接管后把用户鼠标、键盘和中文输入发送到同一个 CDP 页面。

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams, MouseButton,
};
use serde::{Deserialize, Serialize};

/** 用户输入只经面板 IPC，不属于模型可调用的浏览器操作集合。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BrowserUserInput {
    Click {
        x: f64,
        y: f64,
    },
    Scroll {
        x: f64,
        y: f64,
        #[serde(rename = "deltaX")]
        delta_x: f64,
        #[serde(rename = "deltaY")]
        delta_y: f64,
    },
    Text {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: i64,
    },
}

impl BrowserUserInput {
    /** 拒绝非有限坐标和无界文本，输入坐标按固定 CSS 视口约束。 */
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Click { x, y } | Self::Scroll { x, y, .. } => {
                if !x.is_finite()
                    || !y.is_finite()
                    || *x < 0.0
                    || *y < 0.0
                    || *x >= super::session::VIEWPORT_WIDTH as f64
                    || *y >= super::session::VIEWPORT_HEIGHT as f64
                {
                    return Err("点击位置不在浏览器画面内。".to_owned());
                }
                if let Self::Scroll {
                    delta_x, delta_y, ..
                } = self
                {
                    if !delta_x.is_finite()
                        || !delta_y.is_finite()
                        || delta_x.abs() > 10000.0
                        || delta_y.abs() > 10000.0
                    {
                        return Err("滚动距离无效。".to_owned());
                    }
                }
            }
            Self::Text { text } if text.chars().count() > 10000 => {
                return Err("一次输入的文本过长。".to_owned())
            }
            Self::Key { key, modifiers }
                if key.chars().count() > 32 || !(0..=15).contains(modifiers) =>
            {
                return Err("按键无效。".to_owned())
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) async fn send_input(
    page: &chromiumoxide::Page,
    input: BrowserUserInput,
) -> Result<(), String> {
    input.validate()?;
    match input {
        BrowserUserInput::Click { x, y } => {
            for event_type in [
                DispatchMouseEventType::MousePressed,
                DispatchMouseEventType::MouseReleased,
            ] {
                page.execute(
                    DispatchMouseEventParams::builder()
                        .r#type(event_type)
                        .x(x)
                        .y(y)
                        .button(MouseButton::Left)
                        .click_count(1)
                        .build()
                        .map_err(|error| error.to_string())?,
                )
                .await
                .map_err(|error| error.to_string())?;
            }
        }
        BrowserUserInput::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            page.execute(
                DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MouseWheel)
                    .x(x)
                    .y(y)
                    .delta_x(delta_x)
                    .delta_y(delta_y)
                    .build()
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        BrowserUserInput::Text { text } => {
            page.execute(InsertTextParams::new(text))
                .await
                .map_err(|error| error.to_string())?;
        }
        BrowserUserInput::Key { key, modifiers } => {
            // Windows virtualKeyCode 让 Enter、Tab、删除和方向键产生浏览器默认编辑行为。
            let code = match key.as_str() {
                "Backspace" => 8,
                "Tab" => 9,
                "Enter" => 13,
                "Escape" => 27,
                "Delete" => 46,
                "ArrowLeft" => 37,
                "ArrowUp" => 38,
                "ArrowRight" => 39,
                "ArrowDown" => 40,
                "Home" => 36,
                "End" => 35,
                "PageUp" => 33,
                "PageDown" => 34,
                _ => key
                    .chars()
                    .next()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .map(|ch| ch.to_ascii_uppercase() as i64)
                    .unwrap_or(0),
            };
            for event_type in [
                DispatchKeyEventType::RawKeyDown,
                DispatchKeyEventType::KeyUp,
            ] {
                let mut event = DispatchKeyEventParams::new(event_type);
                event.key = Some(key.clone());
                event.windows_virtual_key_code = Some(code);
                event.modifiers = Some(modifiers);
                if modifiers & 2 != 0 && key.eq_ignore_ascii_case("a") {
                    event.commands = Some(vec!["selectAll".to_owned()]);
                }
                page.execute(event)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_coordinates_and_unbounded_input() {
        assert!(BrowserUserInput::Click {
            x: f64::NAN,
            y: 0.0
        }
        .validate()
        .is_err());
        assert!(BrowserUserInput::Click { x: 1280.0, y: 0.0 }
            .validate()
            .is_err());
        assert!(BrowserUserInput::Click { x: 12.0, y: 20.0 }
            .validate()
            .is_ok());
        assert!(BrowserUserInput::Text {
            text: "一".repeat(10001)
        }
        .validate()
        .is_err());
    }
}
