//! Headless native runner for the renderer.
//!
//! Usage: cargo run -p desktop --release -- [--out shot.png] [--w 1920] [--h 1080]
//!        [--t 12.5] [--s 3000] [--speed 0.8] [--x 0] [--seed 1986] [--gl] [--bench 300] [--stress]
//!        [--warm 4]  (seconds of live simulation with autopilot before the shot, so traffic and
//!                     particles are in motion)
//!
//! Renders the same scene the browser does: a `sim::World` placed at track distance `--s`
//! with speed `--speed` (fraction of top speed) and lateral position `--x`, via `game::scene`.

use game::scene::Scene;
use game::vfx::Vfx;
use render::Renderer;
use sim::{TUNING, World};
use std::time::Instant;

struct Args {
    out: String,
    w: u32,
    h: u32,
    t: f32,
    s: f32,
    speed: f32,
    x: f32,
    seed: u64,
    gl: bool,
    bench: u32,
    stress: bool,
    warm: f32,
}

fn parse() -> Args {
    let mut a = Args {
        out: "shot.png".into(),
        w: 1920,
        h: 1080,
        t: 12.5,
        s: 3000.0,
        speed: 0.8,
        x: 2.25,
        seed: 1986,
        gl: false,
        bench: 0,
        stress: false,
        warm: 0.0,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let next = |i: usize| v.get(i + 1).cloned().unwrap_or_default();
        match v[i].as_str() {
            "--out" => a.out = next(i),
            "--w" => a.w = next(i).parse().unwrap_or(a.w),
            "--h" => a.h = next(i).parse().unwrap_or(a.h),
            "--t" => a.t = next(i).parse().unwrap_or(a.t),
            "--s" => a.s = next(i).parse().unwrap_or(a.s),
            "--speed" => a.speed = next(i).parse().unwrap_or(a.speed),
            "--x" => a.x = next(i).parse().unwrap_or(a.x),
            "--seed" => a.seed = next(i).parse().unwrap_or(a.seed),
            "--bench" => a.bench = next(i).parse().unwrap_or(0),
            "--warm" => a.warm = next(i).parse().unwrap_or(0.0),
            "--stress" => {
                a.stress = true;
                i += 1;
                continue;
            }
            "--gl" => {
                a.gl = true;
                i += 1;
                continue;
            }
            other => panic!("unknown arg {other}"),
        }
        i += 2;
    }
    a
}

fn place(world: &mut World, s: f32, x: f32, speed: f32) {
    world.car.s = world.track.wrap(s);
    world.car.x = x;
    world.car.v = speed * TUNING.v_max;
    world.prev = world.car.view();
}

fn main() {
    let a = parse();
    let backends = if a.gl { wgpu::Backends::GL } else { wgpu::Backends::PRIMARY };
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("no adapter");
    let info = adapter.get_info();
    println!("adapter: {} ({:?})", info.name, info.backend);
    // Hold ourselves to the WebGL2 baseline even natively.
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("desktop"),
        required_limits: limits,
        ..Default::default()
    }))
    .expect("device");

    let out_format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let hdr = render::choose_hdr_format(&adapter);
    let mut r = Renderer::new(device.clone(), queue.clone(), hdr, out_format, a.w, a.h);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d { width: a.w, height: a.h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: out_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    let mut world = World::new(a.seed);
    let mut scene = Scene::new();
    let mut vfx = Vfx::new();
    let beat = |t: f32| (t * 118.0 / 60.0).fract();
    place(&mut world, a.s, a.x, a.speed);
    // Re-seed traffic around the new position (World::new populated it near s = 0).
    world.traffic.cars.clear();
    if a.stress {
        world.traffic.row_gap = 28.0;
    }
    world.traffic.populate(&world.track, world.car.s, world.car.v);
    scene.snap(&world);

    // One live frame: autopilot drives (with nitro in stress mode), sim at 120 Hz, VFX updated.
    let live_frame = |world: &mut World, vfx: &mut Vfx, stress: bool| {
        let dt = 1.0 / 60.0;
        for _ in 0..2 {
            let mut input = sim::autopilot(world, sim::LANES[2]);
            input.nitro = stress;
            world.step(&input);
            for e in std::mem::take(&mut world.events) {
                vfx.on_event(&e, world);
            }
        }
        if stress {
            world.car.nitro = 100.0;
            world.car.drift.active = true;
            world.car.drift.tier = 2;
            if vfx.live_particles() < 5000 {
                vfx.debug_fill(world, 5000 - vfx.live_particles());
            }
        }
        vfx.update(world, dt);
    };
    let warm_frames = (a.warm * 60.0) as usize;
    for i in 0..warm_frames {
        live_frame(&mut world, &mut vfx, a.stress);
        // Keep the camera's smoothing state live, as in the browser.
        let _ = scene.build(&world, &mut vfx, 1.0, i as f32 / 60.0, 0.0, 1.0 / 60.0);
    }

    if a.bench > 0 {
        for i in 0..10 {
            let f = scene.build(&world, &mut vfx, 1.0, i as f32 / 60.0, 0.0, 1.0 / 60.0);
            r.render(&view, &f);
        }
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let mut times = Vec::new();
        let mut sim_ms = Vec::new();
        for i in 0..a.bench {
            let t = i as f32 / 60.0;
            let t0 = Instant::now();
            live_frame(&mut world, &mut vfx, a.stress);
            sim_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            let f = scene.build(&world, &mut vfx, 1.0, t, beat(t), 1.0 / 60.0);
            r.render(&view, &f);
            device.poll(wgpu::PollType::wait_indefinitely()).ok();
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        let pct = |v: &mut Vec<f64>, p: f64| {
            v.sort_by(|x, y| x.partial_cmp(y).unwrap());
            v[((v.len() as f64 - 1.0) * p) as usize]
        };
        println!(
            "{}x{} frames={} p50={:.2}ms p95={:.2}ms p99={:.2}ms (sim+vfx p95 {:.2}ms) draws={} passes={} traffic={} particles={}",
            a.w,
            a.h,
            a.bench,
            pct(&mut times, 0.5),
            pct(&mut times, 0.95),
            pct(&mut times, 0.99),
            pct(&mut sim_ms, 0.95),
            r.stats.draw_calls,
            r.stats.passes,
            world.traffic.cars.len(),
            r.stats.particles
        );
    }

    if warm_frames == 0 && a.bench == 0 {
        place(&mut world, a.s, a.x, a.speed);
        scene.snap(&world);
    }
    let f = scene.build(&world, &mut vfx, 1.0, a.t, beat(a.t), 1.0 / 60.0);
    r.render(&view, &f);
    save_png(&device, &queue, &target, a.w, a.h, &a.out);
    let near: Vec<String> = world
        .traffic
        .cars
        .iter()
        .take(6)
        .map(|c| format!("{:?} d{:.0} x{:.1}", c.kind, world.track.delta(c.s, world.car.s), c.x))
        .collect();
    println!(
        "traffic {} instances {} particles {} car v {:.0} km/h  {:?}",
        world.traffic.cars.len(),
        r.stats.instances,
        r.stats.particles,
        world.car.v * 3.6,
        near
    );
    println!("wrote {} (hdr format {:?})", a.out, hdr);
}

fn save_png(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32, path: &str) {
    let bpr = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = buf.slice(..).get_mapped_range().expect("map");
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * bpr) as usize;
        pixels.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    enc.write_header().unwrap().write_image_data(&pixels).unwrap();
}
