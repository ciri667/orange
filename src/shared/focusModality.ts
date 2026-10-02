/** 单独按下这些键不会移动焦点，但 WebView 仍会据此点亮 :focus-visible。 */
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta"]);

const KEYBOARD_CLASS = "using-keyboard";

/** 非修饰键才视为键盘导航。Shift+Tab 的 key 仍是 Tab。 */
export function isKeyboardNavigationKey(key: string) {
  return !MODIFIER_KEYS.has(key);
}

function setKeyboardFocus(active: boolean) {
  document.documentElement.classList.toggle(KEYBOARD_CLASS, active);
}

/**
 * 区分指针操作和键盘导航。
 * 捕获阶段更新，保证随后的焦点样式能读到最新状态。
 */
export function installFocusModality() {
  window.addEventListener(
    "keydown",
    (event) => {
      if (!isKeyboardNavigationKey(event.key)) {
        return;
      }
      setKeyboardFocus(true);
    },
    true,
  );

  window.addEventListener(
    "pointerdown",
    () => {
      setKeyboardFocus(false);
    },
    true,
  );
}
