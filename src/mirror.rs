//! Copias del notch en las demás pantallas ("All displays (mirror)" en la bandeja).
//!
//! Cada copia es una ventana en capas que recibe el MISMO bitmap que el notch
//! principal en cada `render()`: no se dibuja dos veces, solo se presenta en más
//! sitios. Las copias no instalan ganchos de mouse: usan los mensajes normales de
//! su ventana. Al dejar el puntero sobre una copia (o hacer clic), el notch real se
//! muda a esa pantalla y la copia pasa a la pantalla anterior, así se puede usar
//! desde cualquier monitor.
//!
//! Cada copia trae también su propia barra superior (hora, Wi-Fi, volumen,
//! batería...), dibujada con el ancho de SU pantalla para que funcione igual en
//! monitores horizontales y verticales. Pasar el puntero por esa barra también
//! muda el notch real.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::App;
use crate::bar::Bar;
use crate::sys;

/// Tiempo que el puntero debe quedarse sobre una copia antes de mover el notch.
pub const T_MIRROR: usize = 22;
const DWELL_MS: u32 = 200;

pub struct Mirror {
    /// Ventana de la copia del notch.
    pub hwnd: HWND,
    /// Ventana de la barra superior de esta pantalla.
    pub bar_hwnd: HWND,
    pub bar: Option<Bar>,
    pub bar_shown: bool,
    /// Rectángulo de la pantalla donde vive esta copia.
    pub mon: RECT,
    /// Índice de esa pantalla para `sys::pick_monitor` (1 = primera de `sys::monitors()`).
    pub idx: u8,
    pub shown: bool,
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

/// Ventana vacía con la misma clase que el resto (los mensajes se enrutan por handle).
fn create_window() -> Option<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("NockWindow"),
            w!(""), // sin título: los selectores de ventanas (OBS, grabadores) la ignoran
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(hinst),
            None,
        )
        .ok()
    }
}

impl App {
    pub fn is_mirror(&self, h: HWND) -> bool {
        self.mirrors.iter().any(|m| m.hwnd == h || m.bar_hwnd == h)
    }

    /// ¿Hay alguna barra de copia que dibujar? (la hora debe seguir al día aunque
    /// el notch real esté oculto por una app a pantalla completa en su pantalla).
    pub fn mirror_bars_visible(&self) -> bool {
        self.cfg.top_bar && self.mirrors.iter().any(|m| m.shown)
    }

    /// Deja una copia en cada pantalla excepto la del notch real (o ninguna si la
    /// opción está apagada). Se llama al arrancar, al cambiar de pantalla y al
    /// conectar o desconectar monitores.
    pub fn sync_mirrors(&mut self) {
        let mut want: Vec<(u8, RECT)> = Vec::new();
        if self.cfg.mirror {
            for (i, m) in sys::monitors().iter().enumerate() {
                if let Some(r) = sys::monitor_rect(*m) {
                    if !same_rect(&r, &self.mon) {
                        want.push(((i + 1).min(16) as u8, r));
                    }
                }
            }
        }
        while self.mirrors.len() > want.len() {
            if let Some(m) = self.mirrors.pop() {
                unsafe {
                    let _ = DestroyWindow(m.hwnd);
                    let _ = DestroyWindow(m.bar_hwnd);
                }
            }
        }
        while self.mirrors.len() < want.len() {
            match (create_window(), create_window()) {
                (Some(hwnd), Some(bar_hwnd)) => self.mirrors.push(Mirror {
                    hwnd,
                    bar_hwnd,
                    bar: None,
                    bar_shown: false,
                    mon: RECT::default(),
                    idx: 0,
                    shown: false,
                }),
                (a, b) => {
                    unsafe {
                        for h in [a, b].into_iter().flatten() {
                            let _ = DestroyWindow(h);
                        }
                    }
                    break;
                }
            }
        }
        let scale = self.scale;
        for (m, (idx, r)) in self.mirrors.iter_mut().zip(want) {
            m.idx = idx;
            m.mon = r;
            // la barra se rehace con el ancho de esta pantalla (cambia si es vertical)
            m.bar = Bar::new(m.bar_hwnd, r.right - r.left, scale);
        }
        self.mirror_pending = None;
        self.sync_mirror_visibility();
    }

    /// Muestra u oculta cada copia: se ocultan con el notch "guardado" (Ctrl+Alt+H)
    /// y cuando una app a pantalla completa ocupa SU pantalla (para no quedar encima
    /// de un juego o un video en modo ventana sin bordes).
    pub fn sync_mirror_visibility(&mut self) {
        if self.mirrors.is_empty() {
            return;
        }
        let mut skip = vec![self.hwnd];
        if let Some(b) = &self.bar {
            skip.push(b.hwnd);
        }
        if let Some(p) = &self.pop {
            skip.push(p.hwnd);
        }
        skip.extend(self.mirrors.iter().flat_map(|m| [m.hwnd, m.bar_hwnd]));

        let tucked = self.tucked;
        let fs_hide = self.cfg.fs_hide;
        let top_bar = self.cfg.top_bar;
        let mut appeared = false;
        for m in self.mirrors.iter_mut() {
            let show = !tucked && !(fs_hide && sys::fullscreen_on(&skip, &m.mon));
            if show != m.shown {
                m.shown = show;
                appeared |= show;
                unsafe {
                    let _ = ShowWindow(m.hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
                }
            }
            let show_bar = show && top_bar && m.bar.is_some();
            if show_bar != m.bar_shown {
                m.bar_shown = show_bar;
                appeared |= show_bar;
                unsafe {
                    let _ = ShowWindow(m.bar_hwnd, if show_bar { SW_SHOWNOACTIVATE } else { SW_HIDE });
                }
            }
        }
        if appeared {
            // lo recién mostrado necesita su primer cuadro, y el notch debe quedar encima de la barra
            self.render();
            self.render_bar();
            self.raise_windows();
        }
    }

    /// Envía el cuadro ya dibujado a cada copia visible, centrado arriba de su pantalla.
    pub fn present_mirrors(&self, pw: i32, ph: i32) {
        for m in self.mirrors.iter().filter(|m| m.shown) {
            let w = m.mon.right - m.mon.left;
            let _ = self.gfx.present(m.hwnd, m.mon.left + (w - pw) / 2, m.mon.top, pw, ph);
        }
    }

    /// Mensajes de una copia. Solo interesa el puntero: todo lo demás es por defecto.
    pub fn mirror_msg(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEMOVE => {
                if self.mirror_pending != Some(hwnd) {
                    self.mirror_pending = Some(hwnd);
                    self.set_timer(T_MIRROR, DWELL_MS);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                self.jump_to_mirror(hwnd);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    /// Pasó el tiempo de espera: si el puntero sigue sobre la copia, mover el notch ahí.
    pub fn on_mirror_timer(&mut self) {
        self.kill_timer(T_MIRROR);
        let Some(h) = self.mirror_pending.take() else { return };
        let still_inside = unsafe {
            let mut p = POINT::default();
            let mut r = RECT::default();
            GetCursorPos(&mut p).is_ok()
                && GetWindowRect(h, &mut r).is_ok()
                && p.x >= r.left
                && p.x < r.right
                && p.y >= r.top
                && p.y < r.bottom
        };
        if still_inside {
            self.jump_to_mirror(h);
        }
    }

    /// Muda el notch real (y la barra) a la pantalla de esa copia.
    fn jump_to_mirror(&mut self, h: HWND) {
        self.kill_timer(T_MIRROR);
        self.mirror_pending = None;
        let Some(idx) = self.mirrors.iter().find(|m| m.hwnd == h || m.bar_hwnd == h).map(|m| m.idx) else { return };
        sys::log_visibility(&format!("mirror: moving the notch to display {idx}"));
        self.set_monitor(idx); // guarda la pantalla, recoloca todo y rehace las copias
        self.on_foreground(); // re-evalúa pantalla completa en la pantalla nueva
    }

    /// Bandeja: encender o apagar las copias en todas las pantallas.
    pub fn toggle_mirror(&mut self) {
        self.cfg.mirror = !self.cfg.mirror;
        crate::store::save_config(&self.cfg);
        self.sync_mirrors();
        self.render();
        self.render_bar();
    }

    /// Re-evalúa qué barras de copia mostrar (al activar o desactivar la barra superior).
    pub fn sync_mirror_bars(&mut self) {
        let top_bar = self.cfg.top_bar;
        let mut appeared = false;
        for m in self.mirrors.iter_mut() {
            let show_bar = m.shown && top_bar && m.bar.is_some();
            if show_bar != m.bar_shown {
                m.bar_shown = show_bar;
                appeared |= show_bar;
                unsafe {
                    let _ = ShowWindow(m.bar_hwnd, if show_bar { SW_SHOWNOACTIVATE } else { SW_HIDE });
                }
            }
        }
        if appeared {
            self.render_bar();
            self.raise_windows();
        }
    }
}
