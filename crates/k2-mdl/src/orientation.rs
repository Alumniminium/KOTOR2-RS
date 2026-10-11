//! Quaternion and axis-angle conversion utilities for MDL orientation data.
//!
//! Binary MDL stores orientations as quaternions `[w, x, y, z]`.
//! ASCII MDL uses axis-angle representation `x y z angle` (angle in radians).
//! These functions convert between the two representations.

/// Converts a quaternion `[w, x, y, z]` to axis-angle `[ax, ay, az, angle]`.
///
/// The angle is in radians. Identity quaternion `[1, 0, 0, 0]` produces
/// `[0, 0, 0, 0]` (zero rotation).
#[must_use]
pub fn quat_to_axis_angle(q: [f32; 4]) -> [f32; 4] {
    let [w, x, y, z] = q;

    // Normalize the quaternion to handle slight denormalization from f32 math.
    let len = (w * w + x * x + y * y + z * z).sqrt();
    if len < f32::EPSILON {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let w = w / len;
    let x = x / len;
    let y = y / len;
    let z = z / len;

    // Ensure w is in [-1, 1] for acos (clamp for floating-point safety).
    let w_clamped = w.clamp(-1.0, 1.0);
    let angle = 2.0 * w_clamped.acos();

    let sin_half = (1.0 - w_clamped * w_clamped).sqrt();

    if sin_half < 1.0e-6 {
        // Near-zero rotation -- axis is arbitrary, return zero vector.
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [x / sin_half, y / sin_half, z / sin_half, angle]
    }
}

/// Converts axis-angle `[ax, ay, az, angle]` to quaternion `[w, x, y, z]`.
///
/// The angle is in radians. A zero-length axis or zero angle produces the
/// identity quaternion `[1, 0, 0, 0]`.
#[must_use]
pub fn axis_angle_to_quat(aa: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, angle] = aa;

    let axis_len = (ax * ax + ay * ay + az * az).sqrt();
    if axis_len < 1.0e-6 || angle.abs() < 1.0e-6 {
        return [1.0, 0.0, 0.0, 0.0];
    }

    // Normalize the axis.
    let nx = ax / axis_len;
    let ny = ay / axis_len;
    let nz = az / axis_len;

    let half = angle * 0.5;
    let sin_half = half.sin();
    let cos_half = half.cos();

    [cos_half, nx * sin_half, ny * sin_half, nz * sin_half]
}

#[cfg(test)]
mod tests;
