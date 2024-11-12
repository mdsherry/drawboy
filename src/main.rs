use app::MyApp;
use wif::parse;

use eframe::egui;

mod app;
mod ewma;

fn main() -> eframe::Result {
    env_logger::init();
    let w = include_str!(r"houndstooth.wif",);
    let mut wif = parse(w).unwrap();
    wif.build_or_validate_liftplan().unwrap();

    let options = eframe::NativeOptions {
        viewport: if cfg!(feature = "rpi") {
            egui::ViewportBuilder::default().with_fullscreen(true)
        } else {
            egui::ViewportBuilder::default().with_inner_size([1024., 600.])
        },
        ..Default::default()
    };

    eframe::run_native(
        "Drawboy",
        options,
        Box::new(|cc| {
            // This gives us image support:
            // egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(Box::new(MyApp::new(wif, cc)))
        }),
    )
}
