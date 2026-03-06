use std::{f32, fs::File, io::{BufWriter, Write}, time::{Duration, Instant}};

use eframe::egui;
use egui_file_dialog::FileDialog;
use rapl_energy::Rapl;

struct App {
    file_dialog: FileDialog,
    opened_file: Option<BufWriter<File>>,
    last_delta: Instant,
    last_fixed: Instant,
    window_sec: usize,
    fixed_update_hz: usize,
    window_idx: usize,
    cpu_power: Vec<f32>,
    plot_points: Vec<egui_plot::PlotPoint>,
    rapl: Option<Rapl>,
    idle_w: f32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            file_dialog: FileDialog::new().allow_file_overwrite(false),
            opened_file: None,
            last_delta: Instant::now(),
            last_fixed: Instant::now(),
            window_sec: 120,
            fixed_update_hz: 10,
            window_idx: 0,
            cpu_power: vec![0.0; window_capacity(120, 10)],
            plot_points: vec![egui_plot::PlotPoint::new(0.0, 0.0); window_capacity(120, 10)],
            rapl: Rapl::now(false),
            idle_w: f32::MAX,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let fixed_update_dur = Duration::from_secs_f32(1.0 / self.fixed_update_hz as f32);

        let now = Instant::now();
        let delta_time = now.duration_since(self.last_delta);
        let fixed_time = now.duration_since(self.last_fixed);
        self.last_delta = now;

        let first_iteration = self.idle_w == f32::MAX;
        if first_iteration || fixed_time >= fixed_update_dur {
            self.last_fixed = now;
            self.fixed_update(fixed_time);
        }

        self.render(ctx, delta_time);

        ctx.request_repaint_after(fixed_update_dur);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut file) = self.opened_file.take() {
            file.flush().unwrap();
        }
    }
}

impl App {
    fn fixed_update(&mut self, fixed_time: Duration) {
        if let Some(rapl) = &mut self.rapl {
            let energy = rapl.elapsed().into_values().sum::<f32>();
            let power = energy / fixed_time.as_secs_f32();

            if let Some(wtr) = self.opened_file.as_mut() {
                writeln!(wtr, "{}", power).unwrap();
            }

            self.cpu_power[self.window_idx] = power;
            self.window_idx = (self.window_idx + 1) % self.cpu_power.capacity();

            self.idle_w = self.idle_w.min(power);

            rapl.reset();
        }
    }

    fn render(&mut self, ctx: &egui::Context, delta_time: Duration) {
        let cpu_power_max = self.cpu_power.iter().fold(0.0, |x, y| y.max(x));
        let window_max = cpu_power_max - self.idle_w;

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                egui::global_theme_preference_switch(ui);

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
                            self.plot_points = vec![egui_plot::PlotPoint::new(0.0, 0.0); window_capacity(self.window_sec, self.fixed_update_hz)];
                            self.window_idx = 0;
                        }
                    }
                });

                if ui.button("Reset").clicked() {
                    self.idle_w = f32::MAX;
                    for i in 0..window_capacity(self.window_sec, self.fixed_update_hz) {
                        self.cpu_power[i] = 0.0;
                        self.plot_points[i].x = 0.0;
                        self.plot_points[i].y = 0.0;
                    }
                    self.window_idx = 0;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.0} FPS", 1.0 / delta_time.as_secs_f32()));
                });
            });
        });

        self.file_dialog.update(ctx);
        if let Some(path) = self.file_dialog.take_picked() {
            let file = File::create_new(path).unwrap();
            self.opened_file = Some(BufWriter::new(file));
        }

        egui::TopBottomPanel::bottom("stats_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(format!("Found {} RAPL packages", self.rapl.as_ref().map_or(0, |rapl| rapl.packages.len())));

                ui.separator();

                ui.label(format!("Idle: {:.1}W", self.idle_w));
            });
        });

        egui::CentralPanel::default()
            .show(ctx, |ui| {
                let window_elems = window_capacity(self.window_sec, self.fixed_update_hz);

                // TODO: I think we can just create plot_points at the same time as cpu_power and then update in-place
                // Then instead of splicing the array here, we might be better off just creating two lines (or combining two splices?) in `show`

                for x in 0..window_elems {
                    // Map [0,window_elems) to (window_elems,0]
                    let x_inv = window_elems - x - 1;
                    let idx_offset = (x_inv + self.window_idx) % window_elems;
                    let power = self.cpu_power[idx_offset] - self.idle_w;

                    self.plot_points[x].x = x as f64 / self.fixed_update_hz as f64;
                    self.plot_points[x].y = power as f64;
                }

                egui_plot::Plot::new("energy_plot")
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_axis_zoom_drag(false)
                    .default_x_bounds(0f64, self.window_sec as f64)
                    .default_y_bounds(0f64, (window_max as f64 * 1.1).max(1.0))
                    .show(ui, |plot_ui| {
                        let points = egui_plot::PlotPoints::Borrowed(&self.plot_points);
                        plot_ui.line(egui_plot::Line::new("energy_line", points));
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
    eframe::run_native(
        "Energy Monitor",
        eframe::NativeOptions {
            vsync: true,
            ..Default::default()
        },
        Box::new(|_| {
            Ok(Box::<App>::default())
        }),
    )
}
