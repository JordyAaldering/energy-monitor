use std::{f32, time::{Duration, Instant}};
#[cfg(feature = "file-output")]
use std::{fs::File, io::{BufWriter, Write}};

use eframe::egui;
#[cfg(feature = "file-output")]
use egui_file_dialog::FileDialog;
use rapl_energy::Rapl;

#[cfg(feature = "remote-x11")]
const DEFAULT_WINDOW_SEC: usize = 60;
#[cfg(not(feature = "remote-x11"))]
const DEFAULT_WINDOW_SEC: usize = 120;

#[cfg(feature = "remote-x11")]
const DEFAULT_FIXED_UPDATE_HZ: usize = 4;
#[cfg(not(feature = "remote-x11"))]
const DEFAULT_FIXED_UPDATE_HZ: usize = 10;

struct App {
    #[cfg(feature = "file-output")]
    file_dialog: FileDialog,
    #[cfg(feature = "file-output")]
    opened_file: Option<BufWriter<File>>,
    last_delta: Instant,
    last_fixed: Instant,
    next_fixed_deadline: Instant,
    window_sec: usize,
    fixed_update_hz: usize,
    window_idx: usize,
    cpu_power: Vec<f32>,
    plot_points: Vec<egui_plot::PlotPoint>,
    plot_dirty: bool,
    rapl: Option<Rapl>,
    #[cfg(feature = "subtract-idle")]
    idle_w: f32,
    frame_delta: Duration,
    measured_update_hz: f32,
}

impl Default for App {
    fn default() -> Self {
        let now = Instant::now();
        let fixed_update_dur = Duration::from_secs_f32(1.0 / DEFAULT_FIXED_UPDATE_HZ as f32);

        Self {
            #[cfg(feature = "file-output")]
            file_dialog: FileDialog::new().allow_file_overwrite(false),
            #[cfg(feature = "file-output")]
            opened_file: None,
            last_delta: now,
            last_fixed: now,
            next_fixed_deadline: now + fixed_update_dur,
            window_sec: DEFAULT_WINDOW_SEC,
            fixed_update_hz: DEFAULT_FIXED_UPDATE_HZ,
            window_idx: 0,
            cpu_power: vec![0.0; window_capacity(DEFAULT_WINDOW_SEC, DEFAULT_FIXED_UPDATE_HZ)],
            plot_points: Vec::new(),
            plot_dirty: true,
            rapl: Rapl::new(false),
            #[cfg(feature = "subtract-idle")]
            idle_w: f32::MAX,
            frame_delta: Duration::from_secs_f32(1.0 / 60.0),
            measured_update_hz: DEFAULT_FIXED_UPDATE_HZ as f32,
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        #[cfg(feature = "file-output")]
        if let Some(mut file) = self.opened_file.take() {
            let _ = file.flush();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let fixed_update_dur = Duration::from_secs_f32(1.0 / self.fixed_update_hz as f32);
        if self.next_fixed_deadline <= self.last_fixed {
            self.next_fixed_deadline = self.last_fixed + fixed_update_dur;
        }

        #[cfg(feature = "remote-x11")]
        {
            // Ignore all input in remote mode to prevent event storms from driving redraws.
            ctx.input_mut(|input| input.events.clear());
        }

        let now = Instant::now();
        let delta_time = now.duration_since(self.last_delta);
        self.last_delta = now;
        self.frame_delta = delta_time;

        if now >= self.next_fixed_deadline {
            let fixed_time = now.duration_since(self.last_fixed);
            self.last_fixed = now;
            self.fixed_update(fixed_time);

            // Rebase from "now" to avoid bursty catch-up updates after jitter.
            self.next_fixed_deadline = now + fixed_update_dur;
        }

        // Compute delay from a fresh timestamp so render time in this frame does not
        // collapse the requested sleep and cause repaint bursts.
        let repaint_after = self
            .next_fixed_deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        ctx.request_repaint_after(repaint_after);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "remote-x11")]
        {
            ui.scope(|ui| {
                let mut style: egui::Style = ui.style().as_ref().clone();
                // Keep the no-input behavior from disabled UI without visually dimming the app.
                style.visuals.widgets.noninteractive = style.visuals.widgets.inactive;
                style.visuals.disabled_alpha = 1.0;
                ui.set_style(style);

                ui.add_enabled_ui(false, |ui| {
                    self.render(ui, self.frame_delta);
                });
            });
            return;
        }

        #[cfg(not(feature = "remote-x11"))]
        self.render(ui, self.frame_delta);
    }
}

impl App {
    fn fixed_update(&mut self, fixed_time: Duration) {
        if let Some(rapl) = &mut self.rapl {
            let dt = fixed_time.as_secs_f32().max(1e-6);
            let instant_hz = 1.0 / dt;
            self.measured_update_hz = self.measured_update_hz * 0.9 + instant_hz * 0.1;

            let energy = rapl.elapsed().into_values().sum::<f32>();
            let power = energy / dt;

            #[cfg(feature = "file-output")]
            if let Some(wtr) = self.opened_file.as_mut() {
                writeln!(wtr, "{}", power).unwrap();
            }

            self.cpu_power[self.window_idx] = power;
            self.window_idx = (self.window_idx + 1) % self.cpu_power.len();
            self.plot_dirty = true;

            #[cfg(feature = "subtract-idle")]
            {
                self.idle_w = self.idle_w.min(power);
            }

            rapl.reset();
        }
    }

    fn render(&mut self, ui: &mut egui::Ui, _delta_time: Duration) {
        #[cfg(feature = "file-output")]
        let ctx = ui.ctx().clone();
        let cpu_power_max = self.cpu_power.iter().fold(0.0, |x, y| y.max(x));
        #[allow(unused_mut)]
        let mut window_max = cpu_power_max;
        #[cfg(feature = "subtract-idle")]
        {
            window_max -= self.idle_w;
        }

        #[cfg(not(feature = "remote-x11"))]
        egui::Panel::top("menu_bar").show_inside(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                egui::global_theme_preference_switch(ui);

                #[cfg(feature = "file-output")]
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.file_dialog.save_file();
                    }

                    if ui.button("Close").clicked() {
                        if let Some(mut file) = self.opened_file.take() {
                            file.flush().unwrap();
                        }
                    }
                });

                ui.menu_button("Settings", |ui| {
                    let mut window_sec = self.window_sec;
                    let mut fixed_update_hz = self.fixed_update_hz;

                    let resp0 = ui.add(egui::Slider::new(&mut window_sec, 10..=240).step_by(10.0).text("Window (sec)"));

                    let resp1 = ui.add(egui::Slider::new(&mut fixed_update_hz, 1..=60).text("Update (Hz)"));

                    if window_sec != self.window_sec || fixed_update_hz != self.fixed_update_hz {
                        ui.label("Release to update");

                        if resp0.drag_stopped() || resp1.drag_stopped() {
                            self.window_sec = window_sec;
                            self.fixed_update_hz = fixed_update_hz;
                            self.cpu_power = vec![0.0; window_capacity(self.window_sec, self.fixed_update_hz)];
                            self.plot_points.clear();
                            self.window_idx = 0;
                            self.plot_dirty = true;
                            self.last_fixed = Instant::now();
                            self.next_fixed_deadline = self.last_fixed + Duration::from_secs_f32(1.0 / self.fixed_update_hz as f32);
                            self.measured_update_hz = self.fixed_update_hz as f32;
                        }
                    }
                });

                if ui.button("Reset").clicked() {
                    #[cfg(feature = "subtract-idle")]
                    {
                        self.idle_w = f32::MAX;
                    }

                    for i in 0..window_capacity(self.window_sec, self.fixed_update_hz) {
                        self.cpu_power[i] = 0.0;
                    }

                    self.window_idx = 0;
                    self.plot_points.clear();
                    self.plot_dirty = true;
                    self.last_fixed = Instant::now();
                    self.next_fixed_deadline = self.last_fixed + Duration::from_secs_f32(1.0 / self.fixed_update_hz as f32);
                    self.measured_update_hz = self.fixed_update_hz as f32;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.1} Hz", self.measured_update_hz));
                });
            });
        });

        #[cfg(feature = "file-output")]
        {
            self.file_dialog.update(&ctx);
            if let Some(path) = self.file_dialog.take_picked() {
                let file = File::create_new(path).unwrap();
                self.opened_file = Some(BufWriter::new(file));
            }
        }

        #[cfg(not(feature = "remote-x11"))]
        egui::Panel::bottom("stats_bar").show_inside(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(format!("Found {} RAPL packages", self.rapl.as_ref().map_or(0, |rapl| rapl.packages.len())));

                ui.separator();

                #[cfg(feature = "subtract-idle")]
                ui.label(format!("Idle: {:.1}W", self.idle_w));
            });
        });

        egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                let window_elems = window_capacity(self.window_sec, self.fixed_update_hz);

                if self.plot_dirty {
                    self.plot_points.clear();

                    // Keep one plotted point per stored sample so the line is stable across updates.
                    self.plot_points.reserve(window_elems.saturating_sub(self.plot_points.capacity()));

                    for x in 0..window_elems {
                        // Map [0,window_elems) to (window_elems,0]
                        let x_inv = window_elems - x - 1;
                        let idx_offset = (x_inv + self.window_idx) % window_elems;
                        #[allow(unused_mut)]
                        let mut power = self.cpu_power[idx_offset];
                        #[cfg(feature = "subtract-idle")]
                        {
                            power -= self.idle_w;
                        }

                        self.plot_points.push(egui_plot::PlotPoint::new(
                            x as f64 / self.fixed_update_hz as f64,
                            power as f64,
                        ));
                    }

                    self.plot_dirty = false;
                }

                egui_plot::Plot::new("energy_plot")
                    .sense(egui::Sense::empty())
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_double_click_reset(false)
                    .allow_boxed_zoom(false)
                    .allow_axis_zoom_drag(false)
                    .show_x(false)
                    .show_y(false)
                    .show_crosshair(false)
                    .show_grid(egui::Vec2b::new(false, true))
                    .auto_bounds(false)
                    .show(ui, |plot_ui| {
                        let ymax = (window_max as f64 * 1.1).max(1.0);
                        let bounds = egui_plot::PlotBounds::from_min_max(
                            [0.0, 0.0],
                            [self.window_sec as f64, ymax.ceil()],
                        );
                        plot_ui.set_plot_bounds(bounds);

                        let points = egui_plot::PlotPoints::Borrowed(&self.plot_points);
                        plot_ui.line(egui_plot::Line::new("energy_line", points).allow_hover(false));
                    });
            });
    }
}

#[inline(always)]
fn window_capacity(window_sec: usize, fixed_update_hz: usize) -> usize {
    // Every second gets `fixed_update_hz` many updates
    // Both ends are inclusive, so add one
    (window_sec * fixed_update_hz) + 1
}

fn main() -> eframe::Result {
    #[allow(unused_mut)]
    let mut native_options = eframe::NativeOptions {
        vsync: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size((1280.0, 720.0)),
        ..Default::default()
    };

    #[cfg(feature = "remote-x11")]
    {
        native_options.vsync = false;
        native_options.multisampling = 0;
        native_options.hardware_acceleration = eframe::HardwareAcceleration::Off;
    }

    eframe::run_native(
        "Energy Monitor",
        native_options,
        Box::new(|creation_context| {
            let style = egui::Style {
                visuals: egui::Visuals::light(),
                ..Default::default()
            };
            creation_context.egui_ctx.set_global_style(style);
            Ok(Box::<App>::default())
        }),
    )
}
