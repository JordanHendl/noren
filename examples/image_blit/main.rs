//use dashi::driver::command::{BeginDrawing, BlitImage, DrawIndexed};
//use dashi::*;
//use std::time::{Duration, Instant};
//use winit::event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent};
//use winit::event_loop::ControlFlow;
//use winit::platform::run_return::EventLoopExtRunReturn;

fn main() {
//    let device = SelectedDevice::default();
//    println!("Using device {}", device);
//
//    // The GPU context that holds all the data.
//    let mut ctx = gpu::Context::new(&ContextInfo { device }).unwrap();
//
//    const WIDTH: u32 = 1280;
//    const HEIGHT: u32 = 1024;
//    // Database to retrieve the data from. 
//    let db = todo!();
//
//    // Retrieve the image from the database.
//    let fb = todo!(); 
//    let fb_view = ImageView {
//        img: fb,
//        ..Default::default()
//    };
//
//    // Display for windowing
//    let mut display = ctx.make_display(&Default::default()).unwrap();
//
//    let mut ring = ctx
//        .make_command_ring(&CommandQueueInfo2 {
//            debug_name: "cmd",
//            ..Default::default()
//        })
//        .unwrap();
//    let sems = ctx.make_semaphores(2).unwrap();
//    'running: loop {
//        // Reset the allocator
//        allocator.reset();
//
//        // Listen to events
//        let mut should_exit = false;
//        {
//            let event_loop = display.winit_event_loop();
//            event_loop.run_return(|event, _target, control_flow| {
//                *control_flow = ControlFlow::Exit;
//                if let Event::WindowEvent { event, .. } = event {
//                    match event {
//                        WindowEvent::CloseRequested => should_exit = true,
//                        WindowEvent::KeyboardInput {
//                            input:
//                                KeyboardInput {
//                                    virtual_keycode: Some(VirtualKeyCode::Escape),
//                                    state: ElementState::Pressed,
//                                    ..
//                                },
//                            ..
//                        } => should_exit = true,
//                        _ => {}
//                    }
//                }
//            });
//        }
//        if should_exit {
//            break 'running;
//        }
//
//        // Get the next image from the display.
//        let (img, sem, _idx, _good) = ctx.acquire_new_image(&mut display).unwrap();
//
//        ring.record(|list| {
//
//            // Begin render pass & bind pipeline
//            let mut stream = CommandStream::new().begin();
//
//            // Blit the framebuffer to the display's image
//            stream.blit_images(&BlitImage {
//                src: fb,
//                dst: img.img,
//                filter: Filter::Nearest,
//                ..Default::default()
//            });
//
//            // Transition the display image for presentation
//            stream.prepare_for_presentation(img.img);
//
//            stream.end().append(list);
//        })
//        .unwrap();
//        // Submit our recorded commands
//        ring.submit(&SubmitInfo {
//            wait_sems: &[sem],
//            signal_sems: &[sems[0], sems[1]],
//            ..Default::default()
//        })
//        .unwrap();
//
//        // Present the display image, waiting on the semaphore that will signal when our
//        // drawing/blitting is done.
//        ctx.present_display(&display, &[sems[0], sems[1]]).unwrap();
//    }
}
