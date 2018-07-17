extern crate sdl2;

#[macro_use]
extern crate vulkano;

#[macro_use]
extern crate vulkano_shader_derive;

use sdl2::event::Event;
use sdl2::event::WindowEvent;
use sdl2::video::Window;
use sdl2::keyboard::Keycode;

use std::ffi::CString;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::mem;

use vulkano::VulkanObject;
use vulkano::buffer::BufferUsage;
use vulkano::buffer::CpuAccessibleBuffer;
use vulkano::command_buffer::AutoCommandBufferBuilder;
use vulkano::command_buffer::DynamicState;
use vulkano::device::Device;
use vulkano::framebuffer::Framebuffer;
use vulkano::framebuffer::Subpass;
use vulkano::instance::Instance;
use vulkano::instance::RawInstanceExtensions;
use vulkano::instance::PhysicalDevice;
use vulkano::instance::Features;
use vulkano::pipeline::GraphicsPipeline;
use vulkano::pipeline::viewport::Viewport;
use vulkano::swapchain;
use vulkano::swapchain::PresentMode;
use vulkano::swapchain::Surface;
use vulkano::swapchain::SurfaceTransform;
use vulkano::swapchain::Swapchain;
use vulkano::swapchain::AcquireError;
use vulkano::swapchain::SwapchainCreationError;
use vulkano::sync::now;
use vulkano::sync::GpuFuture;

fn main() {
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let window = video_subsystem.window("Window", 800, 600)
        .vulkan()
        .build()
        .unwrap();

    let instance_extensions = window.vulkan_instance_extensions().unwrap();
    let raw_instance_extensions = RawInstanceExtensions::new(instance_extensions.iter().map(
        |&v| CString::new(v).unwrap()
        ));
    let instance = Instance::new(None, raw_instance_extensions, None).unwrap();

    let physical = PhysicalDevice::enumerate(&instance).next().expect("No devices found!");

    let mut dimensions = {
        let (width, height) = window.size();
        [width, height]
    };

    let surface_handle = window.vulkan_create_surface(instance.internal_object()).unwrap();
    
    // Hacky workaround required to compile because window.context() does not implement Send/Sync
    // This however can result in a crash if the window is dropped before the surface.
    let surface = unsafe { Surface::from_raw_surface(instance.clone(), surface_handle, ()) };
    //let surface = unsafe { Surface::from_raw_surface(instance.clone(), surface_handle, window.context()) };

    let surface = Arc::new(surface);

    let queue_family = physical.queue_families().find(|&q| {
        q.supports_graphics() && surface.is_supported(q).unwrap_or(false)
    }).expect("couldn't find a graphical queue family");

    let (device, mut queues) = {
        let device_ext = vulkano::device::DeviceExtensions {
            khr_swapchain: true,
            .. vulkano::device::DeviceExtensions::none()
        };
        Device::new(physical, &Features::none(), &device_ext,
                    [(queue_family, 0.5)].iter().cloned()).expect("failed to create device")
    };

    let queue = queues.next().unwrap();


    let (mut swapchain, mut images) = {
        let caps = surface.capabilities(physical)
            .expect("failed to get surface capabilities");
        let alpha = caps.supported_composite_alpha.iter().next().unwrap();
        dimensions = caps.current_extent.unwrap_or(dimensions);
        let format = caps.supported_formats[0].0;

        Swapchain::new(device.clone(), surface.clone(), caps.min_image_count, format,
                       dimensions, 1, caps.supported_usage_flags, &queue,
                       SurfaceTransform::Identity, alpha, PresentMode::Fifo, true,
                       None).expect("failed to create swapchain")
    };

    let vertex_buffer = {
        #[derive(Debug, Clone)]
        struct Vertex { position: [f32; 2] }
        impl_vertex!(Vertex, position);

        CpuAccessibleBuffer::from_iter(device.clone(), BufferUsage::all(), [
            Vertex { position: [-0.5, -0.25] },
            Vertex { position: [0.0, 0.5] },
            Vertex { position: [0.25, -0.1] }
        ].iter().cloned()).expect("failed to create buffer")
    };

    mod vs {
        #[derive(VulkanoShader)]
        #[ty = "vertex"]
        #[src = "
#version 450
layout(location = 0) in vec2 position;
void main() {
    gl_Position = vec4(position, 0.0, 1.0);
}
"]
        struct Dummy;
    }

    mod fs {
        #[derive(VulkanoShader)]
        #[ty = "fragment"]
        #[src = "
#version 450
layout(location = 0) out vec4 f_color;
void main() {
    f_color = vec4(1.0, 0.0, 0.0, 1.0);
}
"]
        struct Dummy;
    }

    let vs = vs::Shader::load(device.clone()).expect("failed to create shader module");
    let fs = fs::Shader::load(device.clone()).expect("failed to create shader module");

    let render_pass = Arc::new(single_pass_renderpass!(device.clone(),
        attachments: {
            // `color` is a custom name we give to the first and only attachment.
            color: {
                // `load: Clear` means that we ask the GPU to clear the content of this
                // attachment at the start of the drawing.
                load: Clear,
                // `store: Store` means that we ask the GPU to store the output of the draw
                // in the actual image. We could also ask it to discard the result.
                store: Store,
                // `format: <ty>` indicates the type of the format of the image. This has to
                // be one of the types of the `vulkano::format` module (or alternatively one
                // of your structs that implements the `FormatDesc` trait). Here we use the
                // generic `vulkano::format::Format` enum because we don't know the format in
                // advance.
                format: swapchain.format(),
                // TODO:
                samples: 1,
            }
        },
        pass: {
            // We use the attachment named `color` as the one and only color attachment.
            color: [color],
            // No depth-stencil attachment is indicated with empty brackets.
            depth_stencil: {}
        }
    ).unwrap());

    // Before we draw we have to create what is called a pipeline. This is similar to an OpenGL
    // program, but much more specific.
    let pipeline = Arc::new(GraphicsPipeline::start()
        // We need to indicate the layout of the vertices.
        // The type `SingleBufferDefinition` actually contains a template parameter corresponding
        // to the type of each vertex. But in this code it is automatically inferred.
        .vertex_input_single_buffer()
        // A Vulkan shader can in theory contain multiple entry points, so we have to specify
        // which one. The `main` word of `main_entry_point` actually corresponds to the name of
        // the entry point.
        .vertex_shader(vs.main_entry_point(), ())
        // The content of the vertex buffer describes a list of triangles.
        .triangle_list()
        // Use a resizable viewport set to draw over the entire window
        .viewports_dynamic_scissors_irrelevant(1)
        // See `vertex_shader`.
        .fragment_shader(fs.main_entry_point(), ())
        // We have to indicate which subpass of which render pass this pipeline is going to be used
        // in. The pipeline will only be usable from this particular subpass.
        .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
        // Now that our builder is filled, we call `build()` to obtain an actual pipeline.
        .build(device.clone())
        .unwrap());

    let mut framebuffers: Option<Vec<Arc<vulkano::framebuffer::Framebuffer<_,_>>>> = None;

    let mut recreate_swapchain = false;

    let mut previous_frame_end = Box::new(now(device.clone())) as Box<GpuFuture>;



    let mut event_pump = sdl_context.event_pump().unwrap();

    'running: loop {
        previous_frame_end.cleanup_finished();

        if recreate_swapchain {
            dimensions = {
                let (new_width, new_height) = window.size();
                [new_width, new_height]
            };

            let (new_swapchain, new_images) = match swapchain.recreate_with_dimension(dimensions) {
                Ok(r) => r,
                // This error tends to happen when the user is manually resizing the window.
                // Simply restarting the loop is the easiest way to fix this issue.
                Err(SwapchainCreationError::UnsupportedDimensions) => {
                    continue;
                },
                Err(err) => panic!("{:?}", err)
            };

            mem::replace(&mut swapchain, new_swapchain);
            mem::replace(&mut images, new_images);

            framebuffers = None;
            recreate_swapchain = false;
        }

        if framebuffers.is_none() {
            let new_framebuffers = Some(images.iter().map(|image| {
                Arc::new(Framebuffer::start(render_pass.clone())
                         .add(image.clone()).unwrap()
                         .build().unwrap())
            }).collect::<Vec<_>>());
            mem::replace(&mut framebuffers, new_framebuffers);
        }

        let (image_num, acquire_future) = match swapchain::acquire_next_image(swapchain.clone(),
                                                                              None) {
            Ok(r) => r,
            Err(AcquireError::OutOfDate) => {
                recreate_swapchain = true;
                continue;
            },
            Err(err) => panic!("{:?}", err)
        };

        let command_buffer = AutoCommandBufferBuilder::primary_one_time_submit(device.clone(), queue.family()).unwrap()
            .begin_render_pass(framebuffers.as_ref().unwrap()[image_num].clone(), false,
                               vec![[0.0, 0.0, 1.0, 1.0].into()])
            .unwrap()

            .draw(pipeline.clone(),
                DynamicState {
                    line_width: None,
                    viewports: Some(vec![Viewport {
                        origin: [0.0, 0.0],
                        dimensions: [dimensions[0] as f32, dimensions[1] as f32],
                        depth_range: 0.0 .. 1.0,
                    }]),
                    scissors: None,
                },
                vertex_buffer.clone(), (), ())
            .unwrap()

            .end_render_pass()
            .unwrap()

            .build().unwrap();

        let future = previous_frame_end.join(acquire_future)
            .then_execute(queue.clone(), command_buffer).unwrap()
            .then_swapchain_present(queue.clone(), swapchain.clone(), image_num)
            .then_signal_fence_and_flush().unwrap();
        previous_frame_end = Box::new(future) as Box<_>;

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit {..} | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                    break 'running
                },
                Event::Window {win_event: WindowEvent::Resized {..}, ..} => {
                    recreate_swapchain = true
                },
                _ => {}
            }
        }
    }
}