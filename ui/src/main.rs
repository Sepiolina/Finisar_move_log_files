#![windows_subsystem = "windows"]
mod app;
mod settings;
mod worker;


fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([760.0, 660.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Log Archiver",
        options,
        Box::new(|cc| {
            app::apply_style(&cc.egui_ctx);
            Box::new(app::App::new())
        }),
    )
}
