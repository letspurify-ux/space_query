/// Skeleton of the letter "S" as a polyline (centerline).
/// Each consecutive pair of points forms one segment.
const S_PATH: [(f32, f32); 16] = [
    // ── top horizontal bar (right → left) ──
    (0.28, -0.48),
    (0.02, -0.48),
    // ── upper-left curve ──
    (-0.16, -0.45),
    (-0.27, -0.38),
    (-0.31, -0.28),
    (-0.28, -0.18),
    (-0.18, -0.09),
    // ── diagonal crossing ──
    (-0.06, -0.03),
    (0.06, 0.03),
    // ── lower-right curve ──
    (0.18, 0.09),
    (0.28, 0.18),
    (0.31, 0.28),
    (0.27, 0.38),
    (0.16, 0.45),
    // ── bottom horizontal bar (right → left) ──
    (-0.02, 0.48),
    (-0.28, 0.48),
];

fn fill_icon(buffer: &mut [u8], size: usize) {
    let pixel_count = size.saturating_mul(size);
    if buffer.len() != pixel_count.saturating_mul(4) || size == 0 {
        return;
    }

    let size_f = size as f32;
    let aa = safe_div(2.2, size_f);

    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let px = safe_div(x as f32 + 0.5, size_f) * 2.0 - 1.0;
            let py = safe_div(y as f32 + 0.5, size_f) * 2.0 - 1.0;

            // ── Background: Apple-style squircle ──
            let bg_dist = rounded_rect_sdf(px, py, 0.84, 0.84, 0.28);
            let bg_alpha = coverage(bg_dist, aa);
            if bg_alpha <= 0.0 {
                buffer[idx] = 0;
                buffer[idx + 1] = 0;
                buffer[idx + 2] = 0;
                buffer[idx + 3] = 0;
                continue;
            }

            // ── Background gradient: diagonal blue ──
            let diag = ((1.0 - px * 0.4 + py * 0.6) * 0.5).clamp(0.0, 1.0);
            let mut color = [
                lerp(22.0, 48.0, 1.0 - diag),
                lerp(70.0, 120.0, 1.0 - diag),
                lerp(160.0, 210.0, 1.0 - diag),
            ];
            let mut alpha = bg_alpha * 255.0;

            // Subtle radial brightness at centre
            let centre_glow = (1.0 - safe_div((px * px + py * py).sqrt(), 1.2))
                .clamp(0.0, 1.0)
                .powf(2.0);
            blend(
                &mut color,
                &mut alpha,
                [80.0, 150.0, 230.0],
                centre_glow * 0.18 * bg_alpha,
            );

            // ── S letter ──
            let s_dist = s_path_distance(px, py);
            let stroke_hw = 0.098;

            // Shadow layer (offset slightly down-right)
            let shadow_dist = s_path_distance(px - 0.015, py - 0.020);
            let shadow = coverage(shadow_dist - (stroke_hw + 0.02), aa * 1.5);
            blend(
                &mut color,
                &mut alpha,
                [8.0, 24.0, 60.0],
                shadow * 0.50 * bg_alpha,
            );

            // Outer glow
            let outer_glow = coverage(s_dist - (stroke_hw + 0.06), aa);
            blend(
                &mut color,
                &mut alpha,
                [140.0, 200.0, 255.0],
                outer_glow * 0.20 * bg_alpha,
            );

            // Main S body — white with very subtle vertical warm/cool tint
            let s_body = coverage(s_dist - stroke_hw, aa);
            let vert_t = safe_div(py + 0.50, 1.0).clamp(0.0, 1.0);
            let body_color = [
                lerp(245.0, 255.0, vert_t),
                lerp(248.0, 252.0, vert_t),
                255.0,
            ];
            blend(&mut color, &mut alpha, body_color, s_body * bg_alpha);

            // Inner highlight — bright white core for "thickness" feel
            let s_inner = coverage(s_dist - stroke_hw * 0.35, aa);
            blend(
                &mut color,
                &mut alpha,
                [255.0, 255.0, 255.0],
                s_inner * 0.30 * bg_alpha,
            );

            buffer[idx] = color[0].clamp(0.0, 255.0) as u8;
            buffer[idx + 1] = color[1].clamp(0.0, 255.0) as u8;
            buffer[idx + 2] = color[2].clamp(0.0, 255.0) as u8;
            buffer[idx + 3] = alpha.clamp(0.0, 255.0) as u8;
        }
    }
}

/// Minimum distance from point `(px, py)` to the S-letter polyline.
fn s_path_distance(px: f32, py: f32) -> f32 {
    let mut min_d = f32::MAX;
    let len = S_PATH.len();
    if len < 2 {
        return min_d;
    }
    for i in 0..len - 1 {
        let (ax, ay) = S_PATH[i];
        let (bx, by) = S_PATH[i + 1];
        let d = segment_distance(px, py, ax, ay, bx, by);
        if d < min_d {
            min_d = d;
        }
    }
    min_d
}

// ── Primitive helpers ─────────────────────────────────────────────────────

fn blend(base: &mut [f32; 3], alpha: &mut f32, top: [f32; 3], cov: f32) {
    if cov <= 0.0 {
        return;
    }
    let sa = cov.clamp(0.0, 1.0) * 255.0;
    let df = 1.0 - safe_div(sa, 255.0);
    base[0] = base[0] * df + top[0] * safe_div(sa, 255.0);
    base[1] = base[1] * df + top[1] * safe_div(sa, 255.0);
    base[2] = base[2] * df + top[2] * safe_div(sa, 255.0);
    *alpha = alpha.max(sa);
}

fn coverage(distance: f32, aa: f32) -> f32 {
    safe_div(aa - distance, aa).clamp(0.0, 1.0)
}

fn segment_distance(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let apx = px - ax;
    let apy = py - ay;
    let ab_len_sq = abx * abx + aby * aby;
    if ab_len_sq <= f32::EPSILON {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let t = safe_div(apx * abx + apy * aby, ab_len_sq).clamp(0.0, 1.0);
    let cx = ax + abx * t;
    let cy = ay + aby * t;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

fn rounded_rect_sdf(px: f32, py: f32, hx: f32, hy: f32, radius: f32) -> f32 {
    let qx = (px.abs() - (hx - radius)).max(0.0);
    let qy = (py.abs() - (hy - radius)).max(0.0);
    let outside = (qx * qx + qy * qy).sqrt();
    let inside = (px.abs() - (hx - radius))
        .max(py.abs() - (hy - radius))
        .min(0.0);
    outside + inside - radius
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}
