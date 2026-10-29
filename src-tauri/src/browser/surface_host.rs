//! 面板显示意图与原生 HWND 共用一个短锁；显隐和布局不等待浏览器状态锁或 CDP。

use std::sync::Mutex;
use super::win32_surface::ChromiumSurface;

#[derive(Clone, Copy)]
struct Bounds { x: f64, y: f64, width: f64, height: f64, scale: f64 }

#[derive(Default)]
struct State {
    bounds: Option<Bounds>,
    visible: bool,
    tab_available: bool,
    surface: Option<ChromiumSurface>,
}

/** 生命周期独立于 Chromium 会话；启动前、关闭后发来的显示意图都不会丢失。 */
#[derive(Default)]
pub(super) struct SurfaceHost(Mutex<State>);

impl SurfaceHost {
    pub fn set_bounds(&self, x: f64, y: f64, width: f64, height: f64, scale: f64) {
        let mut state = self.0.lock().unwrap();
        state.bounds = (x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite()
            && width >= 24.0 && height >= 24.0 && scale.is_finite() && scale > 0.0)
            .then_some(Bounds { x, y, width, height, scale });
        Self::apply(&mut state);
    }

    pub fn set_visible(&self, visible: bool) {
        let mut state = self.0.lock().unwrap();
        state.visible = visible;
        Self::apply(&mut state);
    }

    pub fn set_tab_available(&self, available: bool) {
        let mut state = self.0.lock().unwrap();
        state.tab_available = available;
        Self::apply(&mut state);
    }

    pub fn attach(&self, surface: ChromiumSurface) {
        let mut state = self.0.lock().unwrap();
        if let Some(previous) = state.surface.take() { previous.detach(); }
        state.surface = Some(surface);
        Self::apply(&mut state);
    }

    pub fn detach(&self) {
        let mut state = self.0.lock().unwrap();
        state.tab_available = false;
        if let Some(surface) = state.surface.take() { surface.detach(); }
    }

    pub fn hwnd(&self) -> Option<isize> {
        self.0.lock().unwrap().surface.as_ref().map(ChromiumSurface::hwnd)
    }

    pub fn focus(&self) {
        let state = self.0.lock().unwrap();
        if state.visible && state.bounds.is_some() && state.tab_available {
            if let Some(surface) = state.surface.as_ref() { surface.focus(); }
        }
    }

    /// 几何和可见性分开：更新 bounds 永远不能隐式重新显示被弹层遮住的窗口。
    fn apply(state: &mut State) {
        if let Some(surface) = state.surface.as_mut() {
            if let Some(bounds) = state.bounds {
                surface.set_bounds(bounds.x, bounds.y, bounds.width, bounds.height, bounds.scale);
                if state.visible && state.tab_available { surface.show(); } else { surface.hide(); }
            } else { surface.hide(); }
        }
    }
}
