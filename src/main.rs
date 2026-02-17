use std::{f32, time::{Duration, Instant}};

use rapl_energy::Rapl;

const FIXED_UPDATE_MS: usize = 100;
const FIXED_UPDATE_SEC: f64 = FIXED_UPDATE_MS as f64 * 0.001;
const FIXED_UPDATE_DURATION: Duration = Duration::from_millis(FIXED_UPDATE_MS as u64);

const WINDOW_SEC: usize = 60;
const WINDOW_DURATION: Duration = Duration::from_secs(WINDOW_SEC as u64);
const WINDOW_ELEMS: usize = (WINDOW_SEC * 1000) / FIXED_UPDATE_MS + 1;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        vsync: true,
        ..Default::default()
    };

    eframe::run_native(
        "Energy Monitor",
        options,
        Box::new(|_cc| {
            Ok(Box::<App>::default())
        }),
    )
}

struct App {
    last_delta: Instant,
    last_fixed: Instant,
    rapl: Option<Rapl>,
    cpu_power: [f32; WINDOW_ELEMS],
    window_idx: usize,
    idle_w: f32,
    max_w: f32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            last_delta: Instant::now(),
            last_fixed: Instant::now(),
            rapl: Rapl::now(false),
            cpu_power: [f32::MIN; WINDOW_ELEMS],
            window_idx: 0,
            idle_w: f32::MAX,
            max_w: 0.0,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let delta_time = now.duration_since(self.last_delta);
        let fixed_time = now.duration_since(self.last_fixed);
        self.last_delta = now;

        let first_iteration = self.idle_w == f32::MAX;
        if fixed_time >= FIXED_UPDATE_DURATION || first_iteration {
            self.last_fixed = now;
            self.fixed_update(fixed_time);
        }

        self.render(ctx, delta_time);

        ctx.request_repaint_after(FIXED_UPDATE_DURATION);
    }
}

impl App {
    fn fixed_update(&mut self, fixed_time: Duration) {
        if let Some(rapl) = &mut self.rapl {
            let energy = rapl.elapsed().into_values().sum::<f32>();
            let power = energy / fixed_time.as_secs_f32();

            self.cpu_power[self.window_idx] = power;
            self.window_idx = (self.window_idx + 1) % WINDOW_ELEMS;

            self.idle_w = self.idle_w.min(power);
            self.max_w = self.max_w.max(power);

            rapl.reset();
        }
    }

    fn render(&mut self, ctx: &egui::Context, delta_time: Duration) {
        let cpu_power_max = self.cpu_power.iter().fold(0.0, |x, y| y.max(x));
        let window_max = cpu_power_max - self.idle_w;

        egui::SidePanel::right("stats_panel")
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.label(format!("Found {} RAPL packages", self.rapl.as_ref().map_or(0, |rapl| rapl.packages.len())));
                ui.label(format!("{} FPS", (1.0 / delta_time.as_secs_f32()).round() as u32));
                ui.label(format!("Idle: {:.1}W", self.idle_w));
                ui.label(format!("Window max: {:.1}W", window_max));
                ui.label(format!("Overall max: {:.1}W", self.max_w - self.idle_w));

                if ui.button("reset").clicked() {
                    self.cpu_power = [f32::MIN; WINDOW_ELEMS];
                    self.window_idx = 0;
                    self.idle_w = f32::MAX;
                    self.max_w = 0.0;
                }
            });

        egui::CentralPanel::default()
            .show(ctx, |ui| {
                let data: egui_plot::PlotPoints = (0..WINDOW_ELEMS).map(|x| {
                    // Map [0,WINDOW_ELEMS) to (WINDOW_ELEMS,0]
                    let x_inv = WINDOW_ELEMS - x - 1;
                    let idx_offset = (x_inv + self.window_idx) % WINDOW_ELEMS;
                    let power = self.cpu_power[idx_offset] - self.idle_w;
                    [x as f64 * FIXED_UPDATE_SEC, power as f64]
                }).collect();

                let line = egui_plot::Line::new("energy_line", data);

                egui_plot::Plot::new("energy_plot")
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_axis_zoom_drag(false)
                    .default_x_bounds(0f64, WINDOW_DURATION.as_secs_f64())
                    .default_y_bounds(0f64, (window_max as f64 * 1.1).max(1.0))
                    .show(ui, |plot_ui| {
                        plot_ui.line(line);
                    });
            });
    }
}
