use std::sync::Mutex;
use std::time::Duration;

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Rect, Runtime, WebviewUrl,
    WebviewWindowBuilder, Window, WindowEvent,
};

const LABEL: &str = "widget";
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 480.0;

#[derive(Default)]
pub struct WidgetState {
    interaction: Mutex<Interaction>,
}

#[derive(Default)]
struct Interaction {
    generation: u64,
    pressed_visible: Option<bool>,
}

impl Interaction {
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn press(&mut self, visible: bool) {
        self.invalidate();
        self.pressed_visible = Some(visible);
    }

    fn should_open_on_release(&mut self, visible: bool) -> bool {
        self.invalidate();
        !self.pressed_visible.take().unwrap_or(visible)
    }
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug)]
struct Placement {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

fn placement(anchor: Bounds, work: Bounds, scale: f64) -> Placement {
    let gap = (8.0 * scale).min(work.width.min(work.height) / 4.0);
    let width = (WIDTH * scale).min((work.width - gap * 2.0).max(1.0));
    let height = (HEIGHT * scale).min((work.height - gap * 2.0).max(1.0));
    let right = work.x + work.width;
    let bottom = work.y + work.height;
    let mut x = anchor.x + anchor.width - width;
    let mut y = anchor.y - height - gap;
    if anchor.x + anchor.width <= work.x {
        x = work.x + gap;
        y = anchor.y + anchor.height - height;
    } else if anchor.x >= right {
        x = right - width - gap;
        y = anchor.y + anchor.height - height;
    } else if anchor.y + anchor.height <= work.y {
        y = work.y + gap;
    }
    x = x.clamp(work.x + gap, (right - width - gap).max(work.x + gap));
    y = y.clamp(work.y + gap, (bottom - height - gap).max(work.y + gap));
    Placement {
        position: PhysicalPosition::new(x.round() as i32, y.round() as i32),
        size: PhysicalSize::new(width.round() as u32, height.round() as u32),
    }
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?widget".into()))
        .title("Usage Viewer")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()?;
    Ok(())
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Ok(mut interaction) = app.state::<WidgetState>().interaction.lock() {
        interaction.invalidate();
        interaction.pressed_visible = None;
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        window.hide()?;
    }
    Ok(())
}

pub fn press<R: Runtime>(app: &AppHandle<R>) {
    let visible = app
        .get_webview_window(LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    let generation = app
        .state::<WidgetState>()
        .interaction
        .lock()
        .map(|mut interaction| {
            interaction.press(visible);
            interaction.generation
        })
        .ok();
    if let Some(generation) = generation {
        dismiss_when_unfocused(app, generation);
    }
}

pub fn toggle<R: Runtime>(
    app: &AppHandle<R>,
    rect: Rect,
    position: PhysicalPosition<f64>,
) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(LABEL) else {
        return Ok(());
    };
    let visible = window.is_visible()?;
    let open = app
        .state::<WidgetState>()
        .interaction
        .lock()
        .map(|mut interaction| interaction.should_open_on_release(visible))
        .unwrap_or(!visible);
    if !open {
        return hide(app);
    }
    let monitor = app
        .monitor_from_point(position.x, position.y)?
        .or(app.primary_monitor()?);
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let icon_position = rect.position.to_physical::<f64>(scale);
        let icon_size = rect.size.to_physical::<f64>(scale);
        let work = monitor.work_area();
        let placement = placement(
            Bounds {
                x: icon_position.x,
                y: icon_position.y,
                width: icon_size.width,
                height: icon_size.height,
            },
            Bounds {
                x: f64::from(work.position.x),
                y: f64::from(work.position.y),
                width: f64::from(work.size.width),
                height: f64::from(work.size.height),
            },
            scale,
        );
        window.set_position(placement.position)?;
        window.set_size(placement.size)?;
        window.set_position(placement.position)?;
    }
    window.show()?;
    window.set_focus()?;
    Ok(())
}

pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != LABEL {
        return;
    }
    let WindowEvent::Focused(focused) = event else {
        return;
    };
    let app = window.app_handle();
    let generation = {
        let state = app.state::<WidgetState>();
        let Ok(mut interaction) = state.interaction.lock() else {
            return;
        };
        interaction.invalidate();
        interaction.generation
    };
    if *focused {
        return;
    }
    dismiss_when_unfocused(app, generation);
}

fn dismiss_when_unfocused<R: Runtime>(app: &AppHandle<R>, generation: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A tray press can deliver focus loss before its mouse event on Windows.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let current = handle
                .state::<WidgetState>()
                .interaction
                .lock()
                .is_ok_and(|interaction| interaction.generation == generation);
            if current {
                if let Some(window) = handle.get_webview_window(LABEL) {
                    if !window.is_focused().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> Bounds {
        Bounds {
            x,
            y,
            width,
            height,
        }
    }

    fn assert_inside(result: &Placement, work: Bounds) {
        assert!(f64::from(result.position.x) >= work.x);
        assert!(f64::from(result.position.y) >= work.y);
        assert!(f64::from(result.position.x) + f64::from(result.size.width) <= work.x + work.width);
        assert!(
            f64::from(result.position.y) + f64::from(result.size.height) <= work.y + work.height
        );
    }

    #[test]
    fn anchors_above_bottom_taskbar_and_below_top_taskbar() {
        let bottom_work = bounds(0.0, 0.0, 1920.0, 1040.0);
        let bottom = placement(bounds(1860.0, 1040.0, 24.0, 40.0), bottom_work, 1.0);
        assert_eq!(bottom.size, PhysicalSize::new(360, 480));
        assert_eq!(bottom.position.y, 552);
        assert_inside(&bottom, bottom_work);

        let top_work = bounds(0.0, 40.0, 1920.0, 1040.0);
        let top = placement(bounds(1860.0, 0.0, 24.0, 40.0), top_work, 1.0);
        assert_eq!(top.position.y, 48);
        assert_inside(&top, top_work);
    }

    #[test]
    fn handles_negative_monitor_coordinates_and_mixed_dpi() {
        let work = bounds(-2560.0, -300.0, 2560.0, 1380.0);
        let result = placement(bounds(-80.0, 1080.0, 36.0, 60.0), work, 1.5);
        assert_eq!(result.size, PhysicalSize::new(540, 720));
        assert_eq!(result.position.y, 348);
        assert_inside(&result, work);
    }

    #[test]
    fn handles_side_taskbars_overflow_icons_and_small_work_areas() {
        let left_work = bounds(48.0, 0.0, 1872.0, 1080.0);
        let left = placement(bounds(0.0, 800.0, 48.0, 32.0), left_work, 1.0);
        assert_eq!(left.position.x, 56);
        assert_inside(&left, left_work);

        let right_work = bounds(0.0, 0.0, 1872.0, 1080.0);
        let right = placement(bounds(1872.0, 800.0, 48.0, 32.0), right_work, 1.0);
        assert_eq!(right.position.x, 1504);
        assert_inside(&right, right_work);

        let small_work = bounds(100.0, 100.0, 400.0, 300.0);
        let overflow = placement(bounds(430.0, 320.0, 32.0, 32.0), small_work, 2.0);
        assert_eq!(overflow.size, PhysicalSize::new(368, 268));
        assert_inside(&overflow, small_work);
    }

    #[test]
    fn tray_release_closes_even_if_focus_loss_already_hid_the_widget() {
        let mut interaction = Interaction::default();
        interaction.press(true);
        interaction.invalidate();
        assert!(!interaction.should_open_on_release(false));
        interaction.press(false);
        assert!(interaction.should_open_on_release(false));
    }

    #[test]
    fn tray_press_and_refocus_invalidate_pending_blur_dismissals() {
        let mut interaction = Interaction::default();
        interaction.invalidate();
        let pending_blur = interaction.generation;
        interaction.press(true);
        assert_ne!(interaction.generation, pending_blur);
        interaction.should_open_on_release(true);
        let before_refocus = interaction.generation;
        interaction.invalidate();
        assert_ne!(interaction.generation, before_refocus);
    }
}
