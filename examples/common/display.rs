use std::error::Error;

use dashi::driver::command::BlitImage;
use dashi::gpu::{CommandStream, Context};
use dashi::{
    CommandQueueInfo2, DisplayInfo, Filter, Handle, Image, SubmitInfo, SubresourceRange, WindowInfo,
};
use winit::event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent};
use winit::event_loop::ControlFlow;
use winit::platform::run_return::EventLoopExtRunReturn;

#[allow(dead_code)]
pub fn blit_image_to_display(
    ctx: &mut Context,
    source: Handle<Image>,
    dims: [u32; 2],
    title: &str,
) -> Result<(), Box<dyn Error>> {
    let mut display = ctx.make_display(&DisplayInfo {
        window: WindowInfo {
            title: title.to_string(),
            size: dims,
            resizable: false,
        },
        vsync: true,
        ..Default::default()
    })?;

    let sems = ctx.make_semaphores(2)?;
    let mut ring = ctx.make_command_ring(&CommandQueueInfo2 {
        debug_name: "cmd",
        ..Default::default()
    })?;

    loop {
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
            break;
        }

        let (img, sem, _idx, _good) = ctx.acquire_new_image(&mut display)?;

        ring.record(|list| {
            CommandStream::new().begin().blit_images(&BlitImage {
                src: source,
                dst: img.img,
                src_range: SubresourceRange::default(),
                dst_range: SubresourceRange::default(),
                filter: Filter::Linear,
                ..Default::default()
            }).prepare_for_presentation(img.img).end().append(list).unwrap();
        })?;

        ring.submit(&SubmitInfo {
            wait_sems: &[sem],
            signal_sems: &[sems[0], sems[1]],
            ..Default::default()
        })?;

        ctx.present_display(&display, &[sems[0], sems[1]])?;
    }

    Ok(())
}
