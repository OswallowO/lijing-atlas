// 麗京原生版著色器（地形、海面、建物、道路、地標網格、軌道路線帶）

struct Globals {
  view_proj: mat4x4<f32>,
  cam_pos: vec4<f32>,     // xyz, w = 時間（秒）
  sun_dir: vec4<f32>,     // xyz, w = 夜間 0..1
  sun_col: vec4<f32>,     // rgb, w = 霧密度
  sky_col: vec4<f32>,     // rgb, w = 年份
  ground_col: vec4<f32>,  // rgb, w = 模式（0 一般、2 等高線）
  fog_col: vec4<f32>,     // rgb, w = 淡化（運輸模式）
  cs2: vec4<f32>,         // 框心 x, y, 半邊長, 旋轉
  cs2b: vec4<f32>,        // 格數, 開關, 熱區開關, 每像素公尺
  tb: vec4<f32>,          // 地形 xmin, xmax, ymin, ymax
  tex: vec4<f32>,         // 1/w, 1/h, 每像素公尺, 畫面高
  conc: vec4<f32>,        // 未包覆外牆色
  glass: vec4<f32>,       // 玻璃色
  heat_b: vec4<f32>,      // 熱區範圍
  type_col: array<vec4<f32>, 12>,
  groups: array<vec4<f32>, 16>, // 群組可見度（地標／路線）
  day: vec4<f32>,               // x 目前日（2014-01-01 起算）、y 時間（秒）
  inv_vp: mat4x4<f32>,          // 天空：螢幕座標反推視線
};
@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var h_tex: texture_2d<f32>;
@group(0) @binding(2) var g_tex: texture_2d<f32>;
@group(0) @binding(3) var c_tex: texture_2d<f32>;
@group(0) @binding(4) var lin_s: sampler;
@group(0) @binding(5) var heat_tex: texture_2d<f32>;
@group(0) @binding(6) var gp_tex: texture_2d<f32>;   // 戰前地表
@group(0) @binding(7) var dev_tex: texture_2d<f32>;  // r 揭露日/18、g 填海日/18、b 舊海岸距離場

fn light(base: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
  let d = max(dot(n, G.sun_dir.xyz), 0.0);
  let amb = mix(G.ground_col.rgb, G.sky_col.rgb, n.y * 0.5 + 0.5);
  return base * (amb * 0.62 + G.sun_col.rgb * d * 0.75);
}
fn fog(c: vec3<f32>, dist: f32) -> vec3<f32> {
  let f = 1.0 - exp(-pow(G.sun_col.w * dist, 2.0));
  return mix(c, G.fog_col.rgb, clamp(f, 0.0, 1.0));
}
fn hash3(p0: vec3<f32>) -> f32 {
  var p = fract(p0 * 0.3183099 + 0.1); p = p * 17.0;
  return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}
fn grid_line(v: f32, w: f32) -> f32 {
  let d = abs(fract(v - 0.5) - 0.5) / max(fwidth(v), 1e-5);
  return 1.0 - clamp(d / w, 0.0, 1.0);
}
fn uv_of(x: f32, z: f32) -> vec2<f32> {
  return vec2<f32>((x - G.tb.x) / (G.tb.y - G.tb.x), (G.tb.w + z) / (G.tb.w - G.tb.z));
}
fn group_vis(g: u32) -> f32 { return G.groups[g / 4u][g % 4u]; }
// sRGB 輸出：目標格式為 sRGB 時由硬體轉換，這裡輸出線性值
fn to_lin(c: vec3<f32>) -> vec3<f32> { return pow(c, vec3<f32>(2.2)); }

// CS2 框：回傳（框內與否, 格線強度）；含 fwidth，須在一致控制流中呼叫
fn cs2_factors(wx: f32, wz: f32, cell_on: bool) -> vec2<f32> {
  var p = vec2<f32>(wx - G.cs2.x, -wz - G.cs2.y);
  let cr = cos(G.cs2.w); let sr = sin(G.cs2.w);
  p = vec2<f32>(p.x * cr + p.y * sr, -p.x * sr + p.y * cr);
  let q = p / (2.0 * G.cs2.z) + 0.5;
  let inside = step(0.0, q.x) * step(q.x, 1.0) * step(0.0, q.y) * step(q.y, 1.0);
  let tl = max(grid_line(q.x * G.cs2b.x, 0.7), grid_line(q.y * G.cs2b.x, 0.7)) * inside;
  let cl = max(grid_line(p.x / 8.0, 0.6), grid_line(p.y / 8.0, 0.6)) * inside * select(0.0, 1.0, cell_on);
  let fr = max(grid_line(q.x, 1.6) * step(0.0, q.y) * step(q.y, 1.0), grid_line(q.y, 1.6) * step(0.0, q.x) * step(q.x, 1.0));
  return vec2<f32>(inside, clamp(tl * 0.55 + cl * 0.25 + fr, 0.0, 1.0));
}
fn cs2_apply(col: vec3<f32>, f: vec2<f32>) -> vec3<f32> {
  let c = mix(col, col * 0.55, (1.0 - f.x) * 0.8);
  return mix(c, to_lin(vec3<f32>(0.05, 0.5, 0.53)), f.y);
}

// ---------------- 地形 ----------------
struct TOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) w: vec3<f32>,
  @location(1) n: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) h: f32,
};
fn H(uv: vec2<f32>) -> f32 { return textureSampleLevel(h_tex, lin_s, uv, 0.0).r; }
fn SD(uv: vec2<f32>) -> f32 {
  // 填海完成前改用舊海岸線
  let dv = textureSampleLevel(dev_tex, lin_s, uv, 0.0);
  let now = (textureSampleLevel(c_tex, lin_s, uv, 0.0).r * 255.0 - 128.0) * 2.0;
  let old = (dv.b * 255.0 - 128.0) * 2.0;
  return select(now, old, dv.g > 0.002 && G.day.x < dv.g * 255.0 * 18.0);
}

@vertex
fn vs_terrain(@location(0) p: vec3<f32>, @location(1) tile: vec3<f32>) -> TOut {
  var o: TOut;
  let xz = tile.xy + p.xz * tile.z;
  let uv = uv_of(xz.x, xz.y);
  var h = H(uv);
  let hx = H(uv + vec2<f32>(G.tex.x, 0.0)) - H(uv - vec2<f32>(G.tex.x, 0.0));
  let hz = H(uv + vec2<f32>(0.0, G.tex.y)) - H(uv - vec2<f32>(0.0, G.tex.y));
  o.n = normalize(vec3<f32>(-hx / (2.0 * G.tex.z), 1.0, -hz / (2.0 * G.tex.z)));
  let sd = SD(uv);
  if (sd < 60.0) { h = min(h, sd); }
  if (sd > 0.0) { h = max(h, min(sd * 0.5, 0.5)); }
  o.h = h;
  if (p.y < -0.5) { h = h - 80.0; }
  o.w = vec3<f32>(xz.x, h, xz.y);
  o.uv = uv;
  o.pos = G.view_proj * vec4<f32>(o.w, 1.0);
  return o;
}
fn hypso(h: f32) -> vec3<f32> {
  var c = vec3<f32>(0.77, 0.84, 0.67);
  c = mix(c, vec3<f32>(0.90, 0.89, 0.70), smoothstep(10.0, 60.0, h));
  c = mix(c, vec3<f32>(0.92, 0.82, 0.62), smoothstep(60.0, 140.0, h));
  c = mix(c, vec3<f32>(0.80, 0.62, 0.42), smoothstep(140.0, 260.0, h));
  c = mix(c, vec3<f32>(0.60, 0.44, 0.36), smoothstep(260.0, 420.0, h));
  return c;
}
@fragment
fn fs_terrain(i: TOut) -> @location(0) vec4<f32> {
  // 逐日地表：揭露日之前顯示戰前地表（道路、公園、都市地坪、填海區都依日期出現）
  let g25 = textureSample(g_tex, lin_s, i.uv).rgb;
  let gpre = textureSample(gp_tex, lin_s, i.uv).rgb;
  let dv = textureSample(dev_tex, lin_s, i.uv);
  let rd = dv.r * 255.0 * 18.0;
  let k = select(1.0, clamp((G.day.x - rd) / 40.0 + 0.5, 0.0, 1.0), dv.r > 0.002);
  var base = mix(gpre, g25, k);
  let n = normalize(i.n);
  let minor = grid_line(i.h / 10.0, 1.0) * step(0.5, i.h);
  let major = grid_line(i.h / 50.0, 1.3) * step(0.5, i.h);
  let huv = vec2<f32>((i.w.x - G.heat_b.x) / (G.heat_b.y - G.heat_b.x), (G.heat_b.w + i.w.z) / (G.heat_b.w - G.heat_b.z));
  let hk = textureSample(heat_tex, lin_s, huv).r;
  let cf = cs2_factors(i.w.x, i.w.z, true);
  if (G.ground_col.w > 1.5) {
    if (i.h > 0.5) { base = mix(base, to_lin(hypso(i.h)), 0.85); }
    base = mix(base, to_lin(vec3<f32>(0.45, 0.30, 0.15)), max(minor * 0.7, major));
  }
  if (G.cs2b.z > 0.5) {
    let hc = mix(vec3<f32>(0.99, 0.91, 0.69), vec3<f32>(0.89, 0.34, 0.30), smoothstep(0.3, 0.9, hk));
    base = mix(base, to_lin(hc), smoothstep(0.08, 0.5, hk) * 0.75);
  }
  var col = light(base, n);
  let night = G.sun_dir.w;
  if (night > 0.0) {
    let lum = dot(base, vec3<f32>(0.3, 0.5, 0.2));
    let urban = smoothstep(0.62, 0.78, lum) * step(0.5, i.h);
    col = mix(col, col * 0.18 + vec3<f32>(0.002, 0.003, 0.006), night) + night * urban * vec3<f32>(0.05, 0.028, 0.009);
  }
  if (G.cs2b.y > 0.5) { col = cs2_apply(col, cf); }
  col = fog(col, length(i.w - G.cam_pos.xyz));
  return vec4<f32>(col, 1.0);
}

// ---------------- 海面 ----------------
struct WOut { @builtin(position) pos: vec4<f32>, @location(0) w: vec3<f32> };
@vertex
fn vs_water(@location(0) p: vec3<f32>) -> WOut {
  var o: WOut; o.w = p; o.pos = G.view_proj * vec4<f32>(p, 1.0); return o;
}
@fragment
fn fs_water(i: WOut) -> @location(0) vec4<f32> {
  let uv = uv_of(i.w.x, i.w.z);
  let outside = uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0;
  var h = -40.0; var sd = -256.0;
  if (!outside) {
    h = min(textureSampleLevel(h_tex, lin_s, uv, 0.0).r, 0.0);
    sd = SD(uv);
  }
  let aa = max(fwidth(sd), 0.5);
  let cf = cs2_factors(i.w.x, i.w.z, false);
  let alpha = clamp(0.5 - sd / aa, 0.0, 1.0);
  if (alpha < 0.01) { discard; }
  h = min(h, sd * 0.6);
  let depth = clamp(-h / 30.0, 0.0, 1.0);
  let base = to_lin(mix(vec3<f32>(0.47, 0.72, 0.78), vec3<f32>(0.16, 0.36, 0.52), sqrt(depth)));
  let t = G.cam_pos.w;
  let wv = sin(i.w.x * 0.05 + t * 0.8) * sin(i.w.z * 0.043 - t * 0.6) + sin((i.w.x + i.w.z) * 0.11 + t * 1.3) * 0.5;
  let n = normalize(vec3<f32>(wv * 0.03, 1.0, wv * 0.02));
  let v = normalize(G.cam_pos.xyz - i.w);
  let spec = pow(max(dot(reflect(-G.sun_dir.xyz, n), v), 0.0), 120.0);
  let fres = pow(1.0 - max(dot(v, vec3<f32>(0.0, 1.0, 0.0)), 0.0), 4.0);
  let night = G.sun_dir.w;
  var col = light(base, n) + G.sun_col.rgb * spec * 0.6 * (1.0 - night) + G.sky_col.rgb * fres * 0.35;
  col = mix(col, col * 0.2 + vec3<f32>(0.0005, 0.001, 0.003), night);
  if (G.cs2b.y > 0.5) { col = cs2_apply(col, cf); }
  col = fog(col, length(i.w - G.cam_pos.xyz));
  return vec4<f32>(col, alpha);
}

// ---------------- 建物 ----------------
// 逐日：開工前不顯示；施工中依進度由底部長高，外觀為鷹架與防塵網；竣工後一般外觀；拆除日之後消失。
struct BOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) w: vec3<f32>,
  @location(1) n: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) @interpolate(flat) m: vec4<u32>,
  @location(4) @interpolate(flat) prog: f32,
  @location(5) @interpolate(flat) cz: u32,
};
@vertex
fn vs_bld(@location(0) p: vec3<f32>, @location(1) n: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) m: vec4<u32>, @location(4) days: vec4<u32>, @location(5) base: f32) -> BOut {
  var o: BOut; o.n = n.xyz; o.uv = uv; o.m = m; o.cz = days.w;
  let day = G.day.x;
  let s = f32(days.x); let e = f32(days.y); let dd = f32(days.z);
  var prog = 1.0;
  if (days.y > 0u) { prog = clamp((day - s) / max(e - s, 1.0), 0.0, 1.0); }
  var q = p;
  if (prog < 1.0) { q.y = base + (p.y - base) * max(prog, 0.02); }
  o.w = q; o.prog = prog;
  o.pos = G.view_proj * vec4<f32>(q, 1.0);
  let hide = (days.y > 0u && day < s) || (days.z > 0u && day >= dd);
  if (hide) { o.pos = vec4<f32>(0.0, 0.0, -2.0, 1.0); }
  return o;
}
fn pal_res(k: u32) -> vec3<f32> {
  // 住宅／公共外牆色票（線性空間前的 sRGB）
  var c = array<vec3<f32>, 8>(vec3<f32>(0.96, 0.95, 0.92), vec3<f32>(0.93, 0.89, 0.80), vec3<f32>(0.86, 0.80, 0.70), vec3<f32>(0.82, 0.83, 0.84),
    vec3<f32>(0.88, 0.74, 0.64), vec3<f32>(0.80, 0.85, 0.78), vec3<f32>(0.76, 0.82, 0.88), vec3<f32>(0.90, 0.88, 0.86));
  return to_lin(c[k % 8u]);
}
fn pal_glass(k: u32) -> vec3<f32> {
  var c = array<vec3<f32>, 6>(vec3<f32>(0.50, 0.64, 0.76), vec3<f32>(0.46, 0.66, 0.66), vec3<f32>(0.62, 0.58, 0.50), vec3<f32>(0.70, 0.74, 0.78), vec3<f32>(0.44, 0.56, 0.70), vec3<f32>(0.52, 0.62, 0.56));
  return to_lin(c[k % 6u]);
}
fn pal_roof(k: u32, ind: bool) -> vec3<f32> {
  var c = array<vec3<f32>, 6>(vec3<f32>(0.62, 0.32, 0.24), vec3<f32>(0.30, 0.31, 0.33), vec3<f32>(0.36, 0.44, 0.52), vec3<f32>(0.48, 0.36, 0.28), vec3<f32>(0.42, 0.48, 0.42), vec3<f32>(0.70, 0.42, 0.30));
  var m = array<vec3<f32>, 3>(vec3<f32>(0.62, 0.66, 0.70), vec3<f32>(0.44, 0.55, 0.66), vec3<f32>(0.70, 0.70, 0.68));
  if (ind) { return to_lin(m[k % 3u]); }
  return to_lin(c[k % 6u]);
}
@fragment
fn fs_bld(i: BOut) -> @location(0) vec4<f32> {
  let ti = i32(i.m.x); let cyv = f32(i.m.y); let roof = i.m.w == 1u;
  let style = i.m.z & 15u; let pal = i.m.z >> 4u; let va = f32(style);
  let year = G.sky_col.w; let night = G.sun_dir.w;
  var base = G.type_col[clamp(ti, 0, 11)].rgb;
  let resi = ti == 1 || ti == 2 || ti == 3 || ti == 4 || ti == 5 || ti == 7 || ti == 10;
  let officeish = ti == 0 || ti == 8;
  if (resi) { base = mix(base, pal_res(pal), 0.55); }
  var glass = G.glass.rgb;
  if (officeish) { glass = mix(glass, pal_glass(pal), 0.6); base = mix(base, pal_res(pal + 3u), 0.25); }
  if (cyv > 0.5 && year < 1900.0 + cyv) { base = G.conc.rgb; }
  let n = normalize(i.n);
  var emis = vec3<f32>(0.0);
  var fh = 3.2; if (officeish) { fh = 4.0; }
  var cw = 3.2; if (ti == 8) { cw = 2.4; } else if (ti == 3) { cw = 4.0; } else if (style % 4u == 2u && resi) { cw = 4.4; }
  let cx = i.uv.x / cw;
  let aa = clamp(1.6 - max(fwidth(cx), fwidth(i.uv.y / fh)) * 2.2, 0.0, 1.0);
  let fl = floor(i.uv.y / fh); let fy = fract(i.uv.y / fh);
  let fx = fract(cx);
  let wall = abs(n.y) < 0.5 && ti != 9 && i.m.w != 2u && i.m.w != 3u && i.m.w != 4u;
  if (wall) {
    // 立面樣式庫：辦公（窗洞／水平帶窗／帷幕／垂直鰭片）、住宅（窗洞／陽台帶／雙併窗／直條窗）
    let pat = style % 4u;
    var win = step(0.28, fy) * step(fy, 0.86) * step(0.14, fx) * step(fx, 0.86);
    var dark = 0.0;
    if (officeish) {
      if (pat == 1u) { win = step(0.22, fy) * step(fy, 0.88); }
      else if (pat == 2u) { win = 1.0 - max(step(0.95, fx), step(0.93, fy)) * 0.85; }
      else if (pat == 3u) { win = step(0.25, fx) * step(fx, 0.75) * step(0.06, fy); dark = (1.0 - step(0.12, fx)) * 0.25; }
    } else if (resi && ti != 3 && ti != 10) {
      if (pat == 1u) { win = step(0.30, fy) * step(fy, 0.84) * step(0.08, fx) * step(fx, 0.92); dark = step(fy, 0.16) * 0.0 + (1.0 - step(0.10, fy)) * 0.35; }
      else if (pat == 2u) { win = step(0.30, fy) * step(fy, 0.82) * (step(0.10, fx) * step(fx, 0.44) + step(0.56, fx) * step(fx, 0.90)); }
      else if (pat == 3u) { win = step(0.06, fy) * step(fx, 0.30) * step(0.10, fx); }
    }
    win = win * step(1.0, fl);
    let shop = (1.0 - step(1.0, fl)) * step(0.12, fy) * step(fy, 0.92) * step(0.06, fx) * step(fx, 0.94) * select(0.0, 1.0, ti == 4 || ti == 1 || ti == 0 || ti == 8);
    let band = select(0.0, 1.0, (fl % 5.0) == 4.0 && ti != 3 && pat != 2u);
    win = max(win * (1.0 - band * 0.8), shop);
    win = mix(0.4 * step(1.0, fl + 0.5), win, aa);
    var glassy = 0.22; if (ti == 8) { glassy = 0.55; } else if (ti == 0 || ti == 1) { glassy = 0.35; }
    if (officeish && pat == 2u) { glassy = 0.7; }
    if (ti == 10 || ti == 11) { glassy = 0.12; }
    base = mix(base, glass, win * glassy);
    base = base * (1.0 - band * 0.12 * aa) * (1.0 - dark * aa);
    var lit = step(0.42, hash3(vec3<f32>(floor(cx), fl, floor(i.w.x * 0.02) + floor(i.w.z * 0.02))));
    let wc = mix(vec3<f32>(1.0, 0.66, 0.3), vec3<f32>(0.62, 0.8, 1.0), step(0.72, hash3(vec3<f32>(fl, floor(cx) * 1.3, va))));
    lit = mix(0.12 + 1.1 * step(0.55, hash3(vec3<f32>(floor(i.w.x / 18.0), floor(i.uv.y / 12.0), floor(i.w.z / 18.0)))), lit, aa);
    emis = win * lit * wc * night * 1.7 + shop * night * vec3<f32>(1.0, 0.72, 0.4) * 1.4;
    // 霓虹招牌（Cyberpunk 風）：商住混合與部分辦公、工廠在夜間有洋紅／青色燈帶
    let neon_on = (ti == 4 || (ti == 6 && pal % 3u == 0u) || ((ti == 0 || ti == 1) && pal % 4u == 1u));
    if (neon_on && night > 0.0) {
      var nc = array<vec3<f32>, 4>(vec3<f32>(1.0, 0.15, 0.6), vec3<f32>(0.1, 0.9, 1.0), vec3<f32>(1.0, 0.55, 0.1), vec3<f32>(0.6, 0.3, 1.0));
      let c_ = nc[pal % 4u];
      let band = step(0.82, fract(i.uv.y / fh)) * step(fl, 3.0) * step(1.0, fl);
      let vert = step(0.93, fract(i.uv.x / 23.0)) * step(i.uv.y, 30.0);
      emis = emis + c_ * (band + vert * 0.8) * night * 2.2 * aa;
    }
    base = base * (0.78 + 0.22 * clamp(i.uv.y / 8.0, 0.0, 1.0));
  } else if (roof) {
    base = base * 0.9;
    emis = vec3<f32>(1.0, 0.72, 0.42) * night * 0.1 * step(0.35, hash3(vec3<f32>(floor(i.w.x * 0.05), floor(i.w.z * 0.05), va)));
  } else if (i.m.w == 3u) {
    base = pal_roof(pal, ti == 6);                     // 斜屋頂瓦／金屬屋面
    if (ti == 6) { base = base * (0.9 + 0.1 * step(0.5, fract(i.w.x * 0.5 + i.w.z * 0.5))); }
  } else if (i.m.w == 4u) {
    base = glass * 1.1;                                 // 冠頂玻璃、天窗
    emis = vec3<f32>(0.9, 0.95, 1.0) * night * 0.5;
  } else if (i.m.w == 2u) {
    base = mix(base, G.conc.rgb, 0.35);   // 女兒牆、屋頂機房
  } else if (i.m.w == 5u) {
    // 霓虹招牌與屋頂看板：白天為彩色面板，夜間發光
    var nc = array<vec3<f32>, 5>(vec3<f32>(1.0, 0.18, 0.62), vec3<f32>(0.1, 0.85, 1.0), vec3<f32>(1.0, 0.6, 0.12), vec3<f32>(0.62, 0.32, 1.0), vec3<f32>(0.95, 0.95, 0.3));
    let c_ = to_lin(nc[pal % 5u]);
    let stripe = 0.75 + 0.25 * step(0.5, fract(i.w.y * 0.35));
    base = c_ * stripe;
    emis = c_ * (0.12 + night * 2.4) * stripe;
  }
  // CS2 分區圖：依分區上色（住宅綠、商業藍、辦公洋紅、工業黃、服務紅）
  if (G.day.z > 0.5) {
    var zc = array<vec3<f32>, 13>(vec3<f32>(0.62, 0.62, 0.62), vec3<f32>(0.70, 0.88, 0.50), vec3<f32>(0.55, 0.80, 0.40), vec3<f32>(0.36, 0.70, 0.32), vec3<f32>(0.30, 0.62, 0.48), vec3<f32>(0.50, 0.62, 0.25), vec3<f32>(0.16, 0.48, 0.20),
      vec3<f32>(0.50, 0.78, 1.00), vec3<f32>(0.08, 0.32, 0.86), vec3<f32>(0.90, 0.66, 0.90), vec3<f32>(0.66, 0.14, 0.66), vec3<f32>(0.96, 0.72, 0.22), vec3<f32>(0.88, 0.40, 0.42));
    base = to_lin(zc[min(i.cz, 12u)]) * (0.85 + 0.15 * select(0.0, 1.0, roof));
    emis = vec3<f32>(0.0);
  }
  if (i.prog < 1.0) {
    // 施工中：灰色鷹架、綠色防塵網格、頂部塔吊色
    let net = step(0.5, fract(i.uv.x / 1.6)) * 0.15;
    base = mix(vec3<f32>(0.42, 0.45, 0.43), vec3<f32>(0.16, 0.36, 0.22), 0.55 + net);
    if (!wall) { base = vec3<f32>(0.55, 0.52, 0.48); }
    emis = vec3<f32>(0.0);
  }
  var col = light(base, n);
  col = mix(col, col * 0.22, night);
  col = mix(col, vec3<f32>(0.85), G.fog_col.w * 0.55);
  let bd = length(i.w - G.cam_pos.xyz);
  let bf = clamp(1.0 - exp(-pow(G.sun_col.w * bd, 2.0)), 0.0, 1.0);
  col = fog(col, bd) + emis * (1.0 - bf * 0.55);   // 夜間燈火穿透霧氣（空中俯瞰的城市燈海）
  return vec4<f32>(col, 1.0);
}

// ---------------- 道路 ----------------
struct ROut { @builtin(position) pos: vec4<f32>, @location(0) w: vec3<f32>, @location(1) c: vec3<f32>, @location(2) lat: f32, @location(3) @interpolate(flat) km: vec2<u32>, @location(4) sk: f32 };
@vertex
fn vs_road(@location(0) p: vec3<f32>, @location(1) c: vec4<f32>, @location(2) m: vec4<u32>) -> ROut {
  var o: ROut; o.c = c.rgb; o.lat = c.a; o.km = vec2<u32>(m.x, m.w); o.sk = 0.0;
  var q = p;
  // 夜間：遠處道路至少保持約 1 像素寬，城市路網的燈光才不會因為次像素而消失
  let night = G.sun_dir.w;
  if (night > 0.01 && m.x <= 5u && abs(c.a - 0.5) > 0.3) {
    let a = f32(m.w) / 65535.0 * 3.14159265;
    let perp = vec3<f32>(sin(a), 0.0, cos(a));
    let px = length(p - G.cam_pos.xyz) * 0.83 / max(G.tex.w, 1.0);
    q = q + perp * sign(c.a - 0.5) * px * 0.55 * night;
  }
  o.w = q;
  o.pos = G.view_proj * vec4<f32>(q, 1.0);
  // 地面以下：隧道段在網格階段已略過；下沉式掘割把深度往鏡頭拉近，讓路面與擋土牆透過地表畫出來
  let th = H(uv_of(q.x, q.z));
  if (q.y < th - 0.5 && q.y > th - 13.0) {
    o.sk = clamp((th - q.y) / 7.0, 0.0, 1.0);
    let q2 = q + normalize(G.cam_pos.xyz - q) * (th - q.y + 3.0);
    let c2 = G.view_proj * vec4<f32>(q2, 1.0);
    o.pos.z = c2.z / c2.w * o.pos.w;
  }
  // 通車日之前、拆除日之後不顯示（戰前舊路在新路網動工前拆除）
  if ((m.y > 0u && G.day.x < f32(m.y)) || (m.z > 0u && G.day.x >= f32(m.z))) { o.pos = vec4<f32>(0.0, 0.0, -2.0, 1.0); }
  return o;
}
@fragment
fn fs_road(i: ROut) -> @location(0) vec4<f32> {
  var col = light(to_lin(i.c), vec3<f32>(0.0, 1.0, 0.0)) * (1.0 - 0.5 * i.sk);   // 掘割：路面在擋土牆陰影中
  let night = G.sun_dir.w;
  col = mix(col, col * 0.3 + vec3<f32>(0.01, 0.008, 0.006), night);
  col = mix(col, vec3<f32>(0.85), G.fog_col.w * 0.5);
  let dist = length(i.w - G.cam_pos.xyz);
  var emis = vec3<f32>(0.0);
  if (night > 0.01 && G.fog_col.w < 0.1) {
    // 夜間路燈：快速道路白色 LED、幹道與次幹道鈉燈橘色、巷道較暗；沿路緣每 32 m 一盞
    let k = i.km.x;
    var lc = vec3<f32>(1.0, 0.52, 0.16); var kb = 0.5;
    if (k <= 1u) { lc = vec3<f32>(0.95, 0.88, 0.72); kb = 1.5; } else if (k <= 3u) { kb = 1.25; } else if (k == 4u) { kb = 0.85; } else if (k >= 7u) { kb = 0.18; }
    let a = f32(i.km.y) / 65535.0 * 3.14159265;
    let along = (i.w.x * cos(a) - i.w.z * sin(a)) / 32.0;
    let aa = clamp(1.0 - fwidth(along) * 1.5, 0.0, 1.0);
    let lamp = mix(0.45, exp(-pow(fract(along) - 0.5, 2.0) * 40.0), aa);
    let edge = mix(0.6, smoothstep(0.35, 0.95, abs(i.lat * 2.0 - 1.0)), aa);
    emis = lc * kb * night * (0.25 + 1.6 * edge * lamp);
  }
  let f = 1.0 - exp(-pow(G.sun_col.w * dist, 2.0));
  col = fog(col, dist) + emis * (1.0 - clamp(f, 0.0, 1.0) * 0.55);
  return vec4<f32>(col, 1.0);
}

// ---------------- 地標／車站／高架橋網格 ----------------
struct MOut { @builtin(position) pos: vec4<f32>, @location(0) w: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32> };
@vertex
fn vs_mesh(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>, @location(3) gy: vec2<u32>) -> MOut {
  var o: MOut; o.w = p; o.n = n; o.c = c;
  o.pos = G.view_proj * vec4<f32>(p, 1.0);
  let vis = group_vis(gy.x);
  if (vis < 0.5 || (f32(gy.y) > 0.5 && G.sky_col.w < f32(gy.y))) { o.pos = vec4<f32>(0.0, 0.0, -2.0, 1.0); }
  return o;
}
@fragment
fn fs_mesh(i: MOut) -> @location(0) vec4<f32> {
  let n = normalize(i.n);
  let base = to_lin(i.c.rgb);
  var col = light(base, n);
  let night = G.sun_dir.w;
  col = mix(col, col * 0.25, night) + base * i.c.a * (0.3 + night * 1.6);
  col = fog(col, length(i.w - G.cam_pos.xyz));
  return vec4<f32>(col, 1.0);
}

// ---------------- 軌道路線帶（固定螢幕寬度＋白框，地下段淡色虛線） ----------------
struct LOut { @builtin(position) pos: vec4<f32>, @location(0) c: vec4<f32>, @location(1) sdk: vec4<f32> };
@vertex
fn vs_line(@location(0) p: vec3<f32>, @location(1) perp: vec2<f32>, @location(2) c: vec4<f32>, @location(3) sdkg: vec4<f32>) -> LOut {
  var o: LOut;
  let g = u32(sdkg.w);
  let vis = group_vis(g);
  // 類別：0 捷運、1 輕軌、2 高鐵、3 幹線公車、4 接駁公車；kind：0 地上、1 地下
  let cat = floor(sdkg.z / 10.0 + 0.01);
  let kind = sdkg.z - cat * 10.0;
  var wpx = select(6.0, 8.0, G.ground_col.w > 0.5 && G.fog_col.w > 0.5);
  var wmin = 5.0;
  if (cat > 0.5 && cat < 1.5) { wpx = 4.5; wmin = 3.5; }
  else if (cat > 1.5 && cat < 2.5) { wpx = 5.0; wmin = 4.0; }
  else if (cat > 2.5 && cat < 3.5) { wpx = 3.4; wmin = 2.6; }
  else if (cat > 3.5) { wpx = 2.2; wmin = 1.8; }
  let w = max(wmin, wpx * G.cs2b.w) * 0.5;
  let q = p + vec3<f32>(perp.x, 0.0, perp.y) * sdkg.x * w;
  o.pos = G.view_proj * vec4<f32>(q, 1.0);
  o.pos.z = o.pos.z - 0.0004 * o.pos.w;
  o.c = vec4<f32>(c.rgb, vis);
  o.sdk = vec4<f32>(sdkg.x, sdkg.y, kind + cat * 10.0, wpx);
  if (vis < 0.1 || G.sky_col.w < c.a * 255.0 + 1900.0) { o.pos = vec4<f32>(0.0, 0.0, -2.0, 1.0); }
  return o;
}
@fragment
fn fs_line(i: LOut) -> @location(0) vec4<f32> {
  let a = abs(i.sdk.x); let edge = 1.0 - 1.6 / max(i.sdk.w, 3.0);
  let cat = floor(i.sdk.z / 10.0 + 0.01); let kind = i.sdk.z - cat * 10.0;
  let night = G.sun_dir.w;
  let casing = mix(vec3<f32>(1.0), vec3<f32>(0.06, 0.09, 0.13), night);
  var c = to_lin(i.c.rgb) * mix(1.0, 1.3, night);
  let unit = max(24.0, G.cs2b.w * 12.0);
  if (kind > 0.5 && cat < 1.5) {
    // 地下段：淡色＋中央虛線
    c = mix(c, to_lin(casing), 0.3);
    if (a < 0.22 && fract(i.sdk.y / unit) < 0.5) { c = to_lin(casing); }
  }
  if (cat > 0.5 && cat < 1.5 && a < 0.3 && fract(i.sdk.y / (unit * 0.6)) < 0.35) { c = to_lin(casing); }      // 輕軌：短白點
  if (cat > 1.5 && cat < 2.5 && a < 0.55 && fract(i.sdk.y / (unit * 0.5)) < 0.5) { c = to_lin(vec3<f32>(0.95)); }   // 高鐵：黑白相間（鐵道符號）
  if (cat > 2.5 && cat < 3.5 && a < 0.25) { c = to_lin(casing); }                                                  // 幹線公車：中央白線
  if (a > edge && cat < 3.5) { c = to_lin(casing); }
  let focus = i.c.a;
  c = mix(mix(to_lin(casing), vec3<f32>(0.5), 0.4), c, clamp(focus, 0.0, 1.0));
  return vec4<f32>(c, 1.0);
}

// ---------- 天空：地平線霧色 → 天頂漸層、太陽光暈、夜間星空與月暈 ----------
struct SOut { @builtin(position) pos: vec4<f32>, @location(0) ndc: vec2<f32> };
@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SOut {
  var o: SOut;
  let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u)) * 2.0 - 1.0;
  o.pos = vec4<f32>(p, 1.0, 1.0);
  o.ndc = p;
  return o;
}
@fragment
fn fs_sky(i: SOut) -> @location(0) vec4<f32> {
  if (G.ground_col.w > 0.5) { return vec4<f32>(G.fog_col.rgb, 1.0); }
  let w = G.inv_vp * vec4<f32>(i.ndc, 1.0, 1.0);
  let dir = normalize(w.xyz / w.w - G.cam_pos.xyz);
  let night = G.sun_dir.w;
  let e = dir.y;
  let zen = G.sky_col.rgb * mix(vec3<f32>(0.5, 0.68, 1.0), vec3<f32>(0.42, 0.5, 0.8), night);
  var col = mix(G.fog_col.rgb, zen, pow(clamp(e, 0.0, 1.0), 0.42));
  let sd = normalize(G.sun_dir.xyz);
  let s = max(dot(dir, sd), 0.0);
  let up = smoothstep(-0.04, 0.03, e);
  // 太陽（白天）／月亮（夜間）
  let disc = pow(s, 1800.0) * 6.0 + pow(s, 90.0) * 0.35 + pow(s, 8.0) * 0.18;
  col += G.sun_col.rgb * disc * up * mix(1.0, 0.35, night);
  // 低角度太陽：地平線暖色暈
  let low = 1.0 - smoothstep(0.05, 0.5, sd.y);
  col += G.sun_col.rgb * pow(s, 3.0) * low * 0.35 * (1.0 - night) * (1.0 - smoothstep(0.0, 0.35, abs(e)));
  // 城市光害：夜間地平線的暖色光穹
  col += vec3<f32>(0.07, 0.04, 0.018) * night * (1.0 - smoothstep(0.0, 0.22, abs(e)));
  // 星空
  if (night > 0.3 && e > 0.02) {
    let q = floor(dir * 420.0);
    let h = hash3(q);
    let tw = 0.6 + 0.4 * sin(G.day.y * 2.0 + h * 50.0);
    col += vec3<f32>(0.9, 0.95, 1.0) * step(0.9975, h) * night * smoothstep(0.02, 0.25, e) * tw * 0.9;
  }
  return vec4<f32>(col, 1.0);
}
