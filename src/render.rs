//! GPU 繪製：地形（分塊 LOD＋高程位移）、海面、建物與道路（背景執行緒建網格）、地標網格、軌道路線帶。
use crate::data::{Data, MeshVertex};
use crate::mesher::{self, BVertex, LVertex, RVertex};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec3, Vec4};
use std::collections::HashMap;
use std::sync::mpsc;
use wgpu::util::DeviceExt;

pub const COLOR_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MSAA: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_col: [f32; 4],
    pub sky_col: [f32; 4],
    pub ground_col: [f32; 4],
    pub fog_col: [f32; 4],
    pub cs2: [f32; 4],
    pub cs2b: [f32; 4],
    pub tb: [f32; 4],
    pub tex: [f32; 4],
    pub conc: [f32; 4],
    pub glass: [f32; 4],
    pub heat_b: [f32; 4],
    pub type_col: [[f32; 4]; 12],
    pub groups: [[f32; 4]; 16],
    pub day: [f32; 4], // 目前日（2014-01-01 起算）、施工動畫時間
}

struct GpuMesh {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    n: u32,
}
pub struct Chunk {
    pub set: u8, // 0 建物（高）1 建物（低）2 道路
    pub cx: f32,
    pub cz: f32,
    state: u8, // 0 未載 1 載入中 2 完成
    meshes: Vec<(u8, GpuMesh)>,
    pub info: Option<mesher::ChunkInfo>,
    last_used: u64,
}

pub struct Target {
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pub size: (u32, u32),
}
impl Target {
    pub fn new(device: &wgpu::Device, w: u32, h: u32) -> Target {
        let mk = |fmt, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage,
                view_formats: &[],
            })
        };
        let color = mk(COLOR_FMT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let msaa = mk(COLOR_FMT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = mk(DEPTH_FMT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        Target {
            color_view: color.create_view(&Default::default()),
            msaa_view: msaa.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            size: (w.max(1), h.max(1)),
        }
    }
}

pub struct Renderer {
    pub globals: Globals,
    gbuf: wgpu::Buffer,
    pub bgl: wgpu::BindGroupLayout,
    pub bg: wgpu::BindGroup,
    bg_pre: wgpu::BindGroup,
    pub use_pre_ground: bool,
    p_terrain: wgpu::RenderPipeline,
    p_water: wgpu::RenderPipeline,
    p_bld: wgpu::RenderPipeline,
    p_road: wgpu::RenderPipeline,
    pub p_mesh: wgpu::RenderPipeline,
    p_line: wgpu::RenderPipeline,
    grids: Vec<(wgpu::Buffer, wgpu::Buffer, u32)>,
    inst: Vec<wgpu::Buffer>,
    inst_n: Vec<u32>,
    tiles: Vec<[f32; 3]>,
    water: wgpu::Buffer,
    scene_vb: wgpu::Buffer,
    scene_n: u32,
    lines: Option<GpuMesh>,
    pub chunks: HashMap<(u8, i32, i32), Chunk>,
    tx: mpsc::Sender<mesher::Job>,
    rx: mpsc::Receiver<mesher::Done>,
    pending: usize,
    frame_no: u64,
    pub quality: f32,
    pub tris: u64,
    pub calls: u32,
    pub sets_on: [bool; 3],
    aux: (wgpu::TextureView, wgpu::TextureView, wgpu::TextureView, wgpu::Sampler, wgpu::TextureView),
}

fn tex2d(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, fmt: wgpu::TextureFormat, bpp: u32, levels: &[Vec<u8>]) -> wgpu::Texture {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (m, data) in levels.iter().enumerate() {
        let (mw, mh) = ((w >> m).max(1), (h >> m).max(1));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t, mip_level: m as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(mw * bpp), rows_per_image: Some(mh) },
            wgpu::Extent3d { width: mw, height: mh, depth_or_array_layers: 1 },
        );
    }
    t
}
fn mips(img: &image::RgbaImage) -> Vec<Vec<u8>> {
    let mut out = vec![img.as_raw().clone()];
    let mut cur = img.clone();
    while cur.width() > 1 || cur.height() > 1 {
        let (w, h) = ((cur.width() / 2).max(1), (cur.height() / 2).max(1));
        cur = image::imageops::resize(&cur, w, h, image::imageops::FilterType::Triangle);
        out.push(cur.as_raw().clone());
    }
    out
}
fn f16(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let e = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let m = x & 0x7fffff;
    if e <= 0 {
        return sign;
    }
    if e >= 31 {
        return sign | 0x7c00;
    }
    sign | ((e as u16) << 10) | ((m >> 13) as u16)
}

pub fn grid_geo(n: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let w = n + 1;
    let mut pos = vec![];
    let mut idx = vec![];
    for j in 0..=n {
        for i in 0..=n {
            pos.push([i as f32 / n as f32, 0.0, j as f32 / n as f32]);
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * w + i;
            idx.extend_from_slice(&[a, a + w, a + 1, a + 1, a + w, a + w + 1]);
        }
    }
    let mut ring = vec![];
    for i in 0..=n { ring.push(i); }
    for j in 1..=n { ring.push(j * w + n); }
    for i in (0..n).rev() { ring.push(n * w + i); }
    for j in (1..n).rev() { ring.push(j * w); }
    let base = pos.len() as u32;
    for &k in &ring {
        let p = pos[k as usize];
        pos.push([p[0], -1.0, p[2]]);
    }
    let rl = ring.len() as u32;
    for r in 0..rl {
        let a = ring[r as usize];
        let b = ring[((r + 1) % rl) as usize];
        let c = base + r;
        let d = base + (r + 1) % rl;
        idx.extend_from_slice(&[a, c, b, b, c, d, a, b, c, b, d, c]);
    }
    (pos, idx)
}

const LODS: [u32; 5] = [256, 128, 64, 32, 16];

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, d: &Data) -> Renderer {
        let tm = &d.meta.terrain;
        // 高程：R16Float
        let half: Vec<u16> = d.hg.data.iter().map(|&v| f16(v)).collect();
        let h_tex = tex2d(device, queue, tm.w as u32, tm.h as u32, wgpu::TextureFormat::R16Float, 2, &[bytemuck::cast_slice(&half).to_vec()]);
        let c_tex = tex2d(device, queue, d.coast.0 as u32, d.coast.1 as u32, wgpu::TextureFormat::R8Unorm, 1, &[d.coast.2.clone()]);
        let g_tex = tex2d(device, queue, d.ground.width(), d.ground.height(), wgpu::TextureFormat::Rgba8UnormSrgb, 4, &mips(&d.ground));
        let gp_tex = tex2d(device, queue, d.ground_pre.width(), d.ground_pre.height(), wgpu::TextureFormat::Rgba8UnormSrgb, 4, &mips(&d.ground_pre));
        let heat = crate::heat::build(d);
        let heat_tex = tex2d(device, queue, heat.0, heat.0, wgpu::TextureFormat::R8Unorm, 1, &[heat.1.clone()]);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: None,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let mut globals: Globals = bytemuck::Zeroable::zeroed();
        globals.tb = [tm.xmin as f32, tm.xmax as f32, tm.ymin as f32, tm.ymax as f32];
        globals.tex = [1.0 / tm.w as f32, 1.0 / tm.h as f32, tm.pxm as f32, 1.0];
        globals.heat_b = heat.2;
        for g in globals.groups.iter_mut() { *g = [1.0; 4]; }
        let gbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("globals"), contents: bytemuck::bytes_of(&globals), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST });
        let tex_entry = |b| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                tex_entry(5),
            ],
        });
        let hv = h_tex.create_view(&Default::default());
        let cv = c_tex.create_view(&Default::default());
        let hev = heat_tex.create_view(&Default::default());
        let mkbg = |gt: &wgpu::Texture| {
            let gv = gt.create_view(&Default::default());
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: gbuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&hv) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&gv) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&cv) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&hev) },
                ],
            })
        };
        let aux = (hv.clone(), g_tex.create_view(&Default::default()), cv.clone(), sampler.clone(), hev.clone());
        let bg = mkbg(&g_tex);
        let bg_pre = mkbg(&gp_tex);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shaders"), source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let mk = |vs: &str, fs: &str, bufs: &[Option<wgpu::VertexBufferLayout>], blend: Option<wgpu::BlendState>, depth_write: bool, bias: i32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers: bufs },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FMT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias != 0 { -1.0 } else { 0.0 }, clamp: 0.0 },
                }),
                multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: COLOR_FMT, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::VertexFormat as F;
        let vb = |stride: u64, step: wgpu::VertexStepMode, attrs: &'static [wgpu::VertexAttribute]| wgpu::VertexBufferLayout { array_stride: stride, step_mode: step, attributes: attrs };
        const T_V: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
        const T_I: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
        const B_A: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Float32x2, 3 => Uint8x4, 4 => Uint16x4, 5 => Float32];
        const R_A: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Uint16x2];
        const M_A: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Uint16x2];
        const L_A: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Float32x4];
        let _ = F::Float32;
        let p_terrain = mk("vs_terrain", "fs_terrain", &[Some(vb(12, wgpu::VertexStepMode::Vertex, &T_V)), Some(vb(12, wgpu::VertexStepMode::Instance, &T_I))], None, true, 0);
        let p_water = mk("vs_water", "fs_water", &[Some(vb(12, wgpu::VertexStepMode::Vertex, &T_V))], Some(wgpu::BlendState::ALPHA_BLENDING), false, 0);
        let p_bld = mk("vs_bld", "fs_bld", &[Some(vb(std::mem::size_of::<BVertex>() as u64, wgpu::VertexStepMode::Vertex, &B_A))], None, true, 0);
        let p_road = mk("vs_road", "fs_road", &[Some(vb(std::mem::size_of::<RVertex>() as u64, wgpu::VertexStepMode::Vertex, &R_A))], None, true, -2);
        let p_mesh = mk("vs_mesh", "fs_mesh", &[Some(vb(std::mem::size_of::<MeshVertex>() as u64, wgpu::VertexStepMode::Vertex, &M_A))], None, true, 0);
        let p_line = mk("vs_line", "fs_line", &[Some(vb(std::mem::size_of::<LVertex>() as u64, wgpu::VertexStepMode::Vertex, &L_A))], None, true, 0);
        // 地形格網
        let mut grids = vec![];
        let mut inst = vec![];
        let s = 256.0 * tm.pxm as f32;
        let nx = (tm.w as f32 / 256.0).ceil() as i32;
        let ny = (tm.h as f32 / 256.0).ceil() as i32;
        let mut tiles = vec![];
        for j in 0..ny {
            for i in 0..nx {
                tiles.push([tm.xmin as f32 + i as f32 * s, -(tm.ymax as f32 - j as f32 * s), s]);
            }
        }
        for &n in LODS.iter() {
            let (p, idx) = grid_geo(n);
            let v = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&p), usage: wgpu::BufferUsages::VERTEX });
            let i = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&idx), usage: wgpu::BufferUsages::INDEX });
            grids.push((v, i, idx.len() as u32));
            inst.push(device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (tiles.len() * 12) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
        let ws = 260000.0f32;
        let water_v: Vec<[f32; 3]> = vec![[-ws, 0.05, -ws], [ws, 0.05, -ws], [ws, 0.05, ws], [-ws, 0.05, -ws], [ws, 0.05, ws], [-ws, 0.05, ws]];
        let water = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&water_v), usage: wgpu::BufferUsages::VERTEX });
        let sv = if d.scene_verts.is_empty() { vec![bytemuck::Zeroable::zeroed(); 3] } else { d.scene_verts.clone() };
        let scene_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scene"), contents: bytemuck::cast_slice(&sv), usage: wgpu::BufferUsages::VERTEX });
        let (tx, rx) = mesher::spawn(d);
        let mut chunks = HashMap::new();
        let ch = d.meta.chunk as f32;
        for c in &d.meta.bld {
            chunks.insert((0, c[0] as i32, c[1] as i32), Chunk { set: 0, cx: (c[0] as f32 + 0.5) * ch, cz: -(c[1] as f32 + 0.5) * ch, state: 0, meshes: vec![], info: None, last_used: 0 });
        }
        for c in &d.meta.roads {
            chunks.insert((2, c[0] as i32, c[1] as i32), Chunk { set: 2, cx: (c[0] as f32 + 0.5) * ch, cz: -(c[1] as f32 + 0.5) * ch, state: 0, meshes: vec![], info: None, last_used: 0 });
        }
        let lines = {
            let (v, i) = mesher::line_ribbons(d);
            if i.is_empty() { None } else {
                Some(GpuMesh {
                    vb: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&v), usage: wgpu::BufferUsages::VERTEX }),
                    ib: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&i), usage: wgpu::BufferUsages::INDEX }),
                    n: i.len() as u32,
                })
            }
        };
        Renderer {
            globals, gbuf, bgl, bg, bg_pre, use_pre_ground: false, p_terrain, p_water, p_bld, p_road, p_mesh, p_line, grids, inst, inst_n: vec![0; LODS.len()], tiles, water,
            scene_n: sv.len() as u32, scene_vb, lines, chunks, tx, rx, pending: 0, frame_no: 0, quality: 1.0, tris: 0, calls: 0, sets_on: [true, true, true], aux,
        }
    }

    /// 每幀：更新地形 LOD 實例、區塊載入／卸載、上傳背景執行緒完成的網格。
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vp: &Mat4, cam: Vec3) {
        self.frame_no += 1;
        let planes = frustum(vp);
        let q = self.quality;
        let mut per: Vec<Vec<[f32; 3]>> = vec![vec![]; LODS.len()];
        for t in &self.tiles {
            let c = Vec3::new(t[0] + t[2] / 2.0, 100.0, t[1] + t[2] / 2.0);
            if !sphere_in(&planes, c, t[2] * 0.75) { continue; }
            let d = ((c.x - cam.x).powi(2) + (c.z - cam.z).powi(2) + (cam.y - 100.0).max(0.0).powi(2)).sqrt() - t[2] * 0.5;
            let li = if d < 2500.0 * q { 0 } else if d < 7000.0 * q { 1 } else if d < 16000.0 * q { 2 } else if d < 32000.0 * q { 3 } else { 4 };
            per[li].push(*t);
        }
        for (i, v) in per.iter().enumerate() {
            self.inst_n[i] = v.len() as u32;
            if !v.is_empty() { queue.write_buffer(&self.inst[i], 0, bytemuck::cast_slice(v)); }
        }
        // 收網格
        let mut uploads = 0;
        while let Ok(done) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if let Some(c) = self.chunks.get_mut(&done.key) {
                c.meshes.clear();
                for (part, m) in done.meshes {
                    if m.indices.is_empty() { continue; }
                    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &m.verts, usage: wgpu::BufferUsages::VERTEX });
                    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&m.indices), usage: wgpu::BufferUsages::INDEX });
                    c.meshes.push((part, GpuMesh { vb, ib, n: m.indices.len() as u32 }));
                }
                c.info = done.info;
                c.state = 2;
                uploads += 1;
            }
            if uploads > 6 { break; }
        }
        // 需求：視距內（依高度調整），由近而遠
        let reach = |set: u8| -> f32 {
            let h = cam.y.max(200.0);
            match set { 0 => (h * 3.2 + 6000.0).min(42000.0) * q, _ => (h * 2.8 + 4000.0).min(30000.0) * q }
        };
        let mut want: Vec<(f32, (u8, i32, i32))> = vec![];
        let fno = self.frame_no;
        for (k, c) in self.chunks.iter_mut() {
            let d = ((c.cx - cam.x).powi(2) + (c.cz - cam.z).powi(2)).sqrt();
            let r = reach(c.set);
            if d < r && self.sets_on[c.set.min(2) as usize] {
                c.last_used = fno;
                if c.state == 0 { want.push((d, *k)); }
            } else if c.state == 2 && d > r * 1.5 && fno - c.last_used > 600 {
                c.meshes.clear();
                c.state = 0;
                c.info = None;
            }
        }
        want.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (_, k) in want.into_iter().take(12usize.saturating_sub(self.pending)) {
            if let Some(c) = self.chunks.get_mut(&k) {
                c.state = 1;
                self.pending += 1;
                let _ = self.tx.send(mesher::Job { key: k });
            }
        }
    }

    pub fn write_globals(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.gbuf, 0, bytemuck::bytes_of(&self.globals));
    }

    pub fn draw(&mut self, encoder: &mut wgpu::CommandEncoder, target: &Target, vp: &Mat4, cam: Vec3, clear: [f64; 3], show_lines: bool, show_scene: bool) {
        let small_reach = (cam.y.max(200.0) * 2.2 + 2500.0).min(9000.0) * self.quality;
        let planes = frustum(vp);
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.msaa_view,
                depth_slice: None,
                resolve_target: Some(&target.color_view),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: 1.0 }), store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &target.depth_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut tris = 0u64;
        let mut calls = 0u32;
        rp.set_bind_group(0, if self.use_pre_ground { &self.bg_pre } else { &self.bg }, &[]);
        // 地形
        rp.set_pipeline(&self.p_terrain);
        for (i, (v, ib, n)) in self.grids.iter().enumerate() {
            if self.inst_n[i] == 0 { continue; }
            rp.set_vertex_buffer(0, v.slice(..));
            rp.set_vertex_buffer(1, self.inst[i].slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..*n, 0, 0..self.inst_n[i]);
            tris += (*n as u64 / 3) * self.inst_n[i] as u64;
            calls += 1;
        }
        // 道路
        rp.set_pipeline(&self.p_road);
        for c in self.chunks.values() {
            if c.set != 2 || c.state != 2 || !self.sets_on[2] { continue; }
            if !sphere_in(&planes, Vec3::new(c.cx, 50.0, c.cz), 1200.0) { continue; }
            for (_, m) in &c.meshes {
                rp.set_vertex_buffer(0, m.vb.slice(..));
                rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..m.n, 0, 0..1);
                tris += m.n as u64 / 3;
                calls += 1;
            }
        }
        // 建物
        rp.set_pipeline(&self.p_bld);
        for c in self.chunks.values() {
            if c.set != 0 || c.state != 2 || !self.sets_on[0] { continue; }
            if !sphere_in(&planes, Vec3::new(c.cx, 120.0, c.cz), 1300.0) { continue; }
            let dc = ((c.cx - cam.x).powi(2) + (c.cz - cam.z).powi(2)).sqrt();
            for (part, m) in &c.meshes {
                if *part == 1 && dc > small_reach { continue; }
                rp.set_vertex_buffer(0, m.vb.slice(..));
                rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..m.n, 0, 0..1);
                tris += m.n as u64 / 3;
                calls += 1;
            }
        }
        if show_scene {
            rp.set_pipeline(&self.p_mesh);
            rp.set_vertex_buffer(0, self.scene_vb.slice(..));
            rp.draw(0..self.scene_n, 0..1);
            tris += self.scene_n as u64 / 3;
            calls += 1;
        }
        if show_lines {
            if let Some(l) = &self.lines {
                rp.set_pipeline(&self.p_line);
                rp.set_vertex_buffer(0, l.vb.slice(..));
                rp.set_index_buffer(l.ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..l.n, 0, 0..1);
                calls += 1;
            }
        }
        // 海面（半透明，最後）
        rp.set_pipeline(&self.p_water);
        rp.set_vertex_buffer(0, self.water.slice(..));
        rp.draw(0..6, 0..1);
        self.tris = tris;
        self.calls = calls + 1;
    }

    /// 另一組全域參數（剖面視窗用）
    pub fn aux_bind(&self, device: &wgpu::Device) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("aux globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let a = &self.aux;
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&a.0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&a.1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&a.2) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&a.3) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&a.4) },
            ],
        });
        (buf, bg)
    }
    /// 只畫一組網格（剖面視窗）
    pub fn draw_mesh_only(&self, encoder: &mut wgpu::CommandEncoder, target: &Target, bg: &wgpu::BindGroup, vb: &wgpu::Buffer, n: u32, clear: [f64; 3]) {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("interior"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.msaa_view,
                depth_slice: None,
                resolve_target: Some(&target.color_view),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: 1.0 }), store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &target.depth_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_bind_group(0, bg, &[]);
        rp.set_pipeline(&self.p_mesh);
        rp.set_vertex_buffer(0, vb.slice(..));
        rp.draw(0..n, 0..1);
    }

    pub fn loaded_chunks(&self) -> usize {
        self.chunks.values().filter(|c| c.state == 2).count()
    }
}

pub fn frustum(m: &Mat4) -> [Vec4; 6] {
    let r = |i: usize| m.row(i);
    let mut p = [r(3) + r(0), r(3) - r(0), r(3) + r(1), r(3) - r(1), r(2), r(3) - r(2)];
    for q in p.iter_mut() {
        let l = q.truncate().length();
        *q /= l;
    }
    p
}
pub fn sphere_in(p: &[Vec4; 6], c: Vec3, r: f32) -> bool {
    p.iter().all(|q| q.truncate().dot(c) + q.w > -r)
}
