//! Run with `cargo run --example image_blit` to record a simple blit command
//! that copies a database image into a fresh framebuffer.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_TEXTURE_ENTRY, init_context, open_sample_db};
use dashi::driver::command::BlitImage;
use dashi::gpu::CommandStream;
use dashi::gpu::driver::state::SubresourceRange;
use dashi::{CommandQueueInfo2, DisplayInfo, Filter, Rect2D, SubmitInfo, WindowInfo};
use std::error::Error;
use winit::event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent};
use winit::event_loop::ControlFlow;
use winit::platform::run_return::EventLoopExtRunReturn;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut ctx = match init_context() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("Skipping example – unable to create GPU context: {err}");
            return Ok(());
        }
    };

    let mut db = open_sample_db(&mut ctx)?;
    let imagery = db.imagery_mut();

    let source_image = imagery.fetch_gpu_image(SAMPLE_TEXTURE_ENTRY)?;
    let source_handle = source_image.img;

    let mut display = ctx
        .make_display(&DisplayInfo {
            window: WindowInfo {
                title: "image_blit".to_string(),
                size: [source_image.info.dim[0], source_image.info.dim[1]],
                resizable: false,
            },
            vsync: true,
            ..Default::default()
        })
        .expect("Error making Display!");

    let sems = ctx.make_semaphores(2).unwrap();
    let mut ring = ctx
        .make_command_ring(&CommandQueueInfo2 {
            debug_name: "cmd",
            ..Default::default()
        })
        .unwrap();


    let dims = source_image.info.dim;
    println!(
        "Blitting image '{}' ({}x{})",
        SAMPLE_TEXTURE_ENTRY, dims[0], dims[1]
    );

    'running: loop {
        // Listen to events
        let mut should_exit = false;
        {
            let event_loop = display.winit_event_loop();
            event_loop.run_return(|event, _target, control_flow| {
                *control_flow = ControlFlow::Exit;
                if let Event::WindowEvent { event, .. } = event {
                    match event {
                        WindowEvent::CloseRequested => should_exit = true,
                        WindowEvent::KeyboardInput {
                            input:
                                KeyboardInput {
                                    virtual_keycode: Some(VirtualKeyCode::Escape),
                                    state: ElementState::Pressed,
                                    ..
                                },
                            ..
                        } => should_exit = true,
                        _ => {}
                    }
                }
            });
        }
        if should_exit {
            break 'running;
        }

        // Get the next image from the display.
        let (img, sem, _idx, _good) = ctx.acquire_new_image(&mut display).unwrap();

        ring.record(|list| {
            let mut stream = CommandStream::new().begin();

            stream.blit_images(&BlitImage {
                src: source_handle,
                dst: img.img,
                src_range: SubresourceRange::default(),
                dst_range: SubresourceRange::default(),
                filter: Filter::Linear,
                ..Default::default()
            });

            // Transition the display image for presentation
            stream.prepare_for_presentation(img.img);
            stream.end().append(list);
        })
        .unwrap();
        // Submit our recorded commands
        ring.submit(&SubmitInfo {
            wait_sems: &[sem],
            signal_sems: &[sems[0], sems[1]],
            ..Default::default()
        })
        .unwrap();

        // Present the display image, waiting on the semaphore that will signal when our
        // drawing/blitting is done.
        ctx.present_display(&display, &[sems[0], sems[1]]).unwrap();
    }


    Ok(())
}
