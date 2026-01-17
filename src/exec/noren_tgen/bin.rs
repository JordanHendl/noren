mod editor;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_title("Noren Terrain Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Noren Terrain Editor",
        options,
        Box::new(|_| Box::new(editor::TerrainEditorApp::default())),
    )
}
