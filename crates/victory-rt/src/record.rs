//! The two records every part of the validation shares, little-endian and
//! fixed: a ray (48 bytes, twelve 32-bit words) and a hit (32 bytes, eight
//! words). The flag values are D3D12's (`D3D12_RAY_FLAG_*`,
//! `D3D12_RAYTRACING_INSTANCE_FLAG_*`), written out here because this crate
//! depends on nothing.

/// `D3D12_RAY_FLAG` bit values.
pub mod ray_flag {
    pub const FORCE_OPAQUE: u32 = 0x01;
    pub const FORCE_NON_OPAQUE: u32 = 0x02;
    pub const ACCEPT_FIRST_HIT_AND_END_SEARCH: u32 = 0x04;
    pub const SKIP_CLOSEST_HIT_SHADER: u32 = 0x08;
    pub const CULL_BACK_FACING_TRIANGLES: u32 = 0x10;
    pub const CULL_FRONT_FACING_TRIANGLES: u32 = 0x20;
    pub const CULL_OPAQUE: u32 = 0x40;
    pub const CULL_NON_OPAQUE: u32 = 0x80;
    pub const SKIP_TRIANGLES: u32 = 0x100;
    pub const SKIP_PROCEDURAL_PRIMITIVES: u32 = 0x200;
    /// Every bit defined above.
    pub const ALL: u32 = 0x3FF;
}

/// `D3D12_RAYTRACING_INSTANCE_FLAG` bit values.
pub mod instance_flag {
    pub const TRIANGLE_CULL_DISABLE: u32 = 0x1;
    pub const TRIANGLE_FRONT_COUNTERCLOCKWISE: u32 = 0x2;
    pub const FORCE_OPAQUE: u32 = 0x4;
    pub const FORCE_NON_OPAQUE: u32 = 0x8;
    pub const ALL: u32 = 0xF;
}

/// The `kind` word of a [`Hit`].
pub mod hit_kind {
    pub const MISS: u32 = 0;
    pub const FRONT: u32 = 1;
    pub const BACK: u32 = 2;
    /// Reserved for procedural primitives; nothing here produces it.
    pub const PROCEDURAL: u32 = 3;
}

/// A ray. The direction need not be normalised: `t` counts in units of its
/// length, so `t` is the same number in world space and in an instance's
/// object space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: [f32; 3],
    pub tmin: f32,
    pub dir: [f32; 3],
    pub tmax: f32,
    /// The instance mask; the low 8 bits are used.
    pub mask: u32,
    /// [`ray_flag`] bits.
    pub flags: u32,
}

impl Ray {
    pub const SIZE: usize = 48;
    pub const WORDS: usize = 12;

    pub fn new(origin: [f32; 3], dir: [f32; 3], tmin: f32, tmax: f32) -> Self {
        Ray {
            origin,
            tmin,
            dir,
            tmax,
            mask: 0xFF,
            flags: 0,
        }
    }

    pub fn to_words(&self) -> [u32; 12] {
        [
            self.origin[0].to_bits(),
            self.origin[1].to_bits(),
            self.origin[2].to_bits(),
            self.tmin.to_bits(),
            self.dir[0].to_bits(),
            self.dir[1].to_bits(),
            self.dir[2].to_bits(),
            self.tmax.to_bits(),
            self.mask,
            self.flags,
            0,
            0,
        ]
    }

    /// The reserved words are not kept.
    pub fn from_words(w: &[u32; 12]) -> Self {
        let f = |k: usize| f32::from_bits(w[k]);
        Ray {
            origin: [f(0), f(1), f(2)],
            tmin: f(3),
            dir: [f(4), f(5), f(6)],
            tmax: f(7),
            mask: w[8],
            flags: w[9],
        }
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        let mut out = [0u8; 48];
        for (k, w) in self.to_words().iter().enumerate() {
            out[4 * k..4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8; 48]) -> Self {
        let w: [u32; 12] = std::array::from_fn(|k| {
            u32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
        });
        Ray::from_words(&w)
    }

    /// What D3D12 asks of a ray, and what the reference relies on: finite
    /// origin and direction, a direction that is not zero, `0 <= tmin <=
    /// tmax` with no NaN (`tmax` may be infinite; an infinite `tmin`, which
    /// D3D12 allows and which hits nothing, is not modelled), known flag
    /// bits, and none of the combinations D3D12 calls mutually exclusive.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.origin.iter().chain(&self.dir).all(|x| x.is_finite()) {
            return Err("a ray origin or direction that is not finite");
        }
        if self.dir == [0.0; 3] {
            return Err("a ray with no direction");
        }
        // Written so that a NaN fails.
        if !(self.tmin >= 0.0 && self.tmin <= self.tmax && self.tmin.is_finite()) {
            return Err("a ray whose tmin and tmax are not 0 <= tmin <= tmax");
        }
        if self.flags & !ray_flag::ALL != 0 {
            return Err("a ray flag this crate does not know");
        }
        let opacity = ray_flag::FORCE_OPAQUE
            | ray_flag::FORCE_NON_OPAQUE
            | ray_flag::CULL_OPAQUE
            | ray_flag::CULL_NON_OPAQUE;
        if (self.flags & opacity).count_ones() > 1 {
            return Err("more than one of the force and cull opacity ray flags");
        }
        let facing = ray_flag::CULL_BACK_FACING_TRIANGLES | ray_flag::CULL_FRONT_FACING_TRIANGLES;
        if self.flags & facing == facing {
            return Err("both facing cull ray flags");
        }
        let skip = ray_flag::SKIP_TRIANGLES | ray_flag::SKIP_PROCEDURAL_PRIMITIVES;
        if self.flags & skip == skip {
            return Err("both skip ray flags");
        }
        if self.flags & ray_flag::SKIP_TRIANGLES != 0 && self.flags & facing != 0 {
            return Err("the skip-triangles ray flag with a facing cull");
        }
        Ok(())
    }
}

/// What a ray hit. `u` and `v` are the barycentric weights of the
/// triangle's vertices 1 and 2 (the DXR convention). On a miss every word
/// but `kind` is 0, and `kind` is 0 too.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub u: f32,
    pub v: f32,
    /// [`hit_kind`].
    pub kind: u32,
    /// The instance's position in the top-level list.
    pub instance_index: u32,
    /// The instance's 24-bit id.
    pub instance_id: u32,
    pub geometry_index: u32,
    pub primitive_index: u32,
}

impl Hit {
    pub const SIZE: usize = 32;
    pub const WORDS: usize = 8;
    pub const MISS: Hit = Hit {
        t: 0.0,
        u: 0.0,
        v: 0.0,
        kind: hit_kind::MISS,
        instance_index: 0,
        instance_id: 0,
        geometry_index: 0,
        primitive_index: 0,
    };

    pub fn is_miss(&self) -> bool {
        self.kind == hit_kind::MISS
    }

    /// Instance index, instance id, geometry index and primitive index: what
    /// names the triangle.
    pub fn ids(&self) -> [u32; 4] {
        [
            self.instance_index,
            self.instance_id,
            self.geometry_index,
            self.primitive_index,
        ]
    }

    pub fn to_words(&self) -> [u32; 8] {
        [
            self.t.to_bits(),
            self.u.to_bits(),
            self.v.to_bits(),
            self.kind,
            self.instance_index,
            self.instance_id,
            self.geometry_index,
            self.primitive_index,
        ]
    }

    pub fn from_words(w: &[u32; 8]) -> Self {
        Hit {
            t: f32::from_bits(w[0]),
            u: f32::from_bits(w[1]),
            v: f32::from_bits(w[2]),
            kind: w[3],
            instance_index: w[4],
            instance_id: w[5],
            geometry_index: w[6],
            primitive_index: w[7],
        }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (k, w) in self.to_words().iter().enumerate() {
            out[4 * k..4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8; 32]) -> Self {
        let w: [u32; 8] = std::array::from_fn(|k| {
            u32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
        });
        Hit::from_words(&w)
    }
}

/// Word for word: two NaNs with the same bits are the same record.
impl PartialEq for Hit {
    fn eq(&self, other: &Self) -> bool {
        self.to_words() == other.to_words()
    }
}

impl Eq for Hit {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_is_twelve_words_in_the_stated_order() {
        let mut ray = Ray::new([1.0, 2.0, 3.0], [-1.0, 0.5, 0.25], 0.125, 100.0);
        ray.mask = 0x1_0F;
        ray.flags = ray_flag::CULL_BACK_FACING_TRIANGLES;
        let b = ray.to_bytes();
        assert_eq!(b.len(), Ray::SIZE);
        assert_eq!(&b[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&b[12..16], &0.125f32.to_le_bytes());
        assert_eq!(&b[16..20], &(-1.0f32).to_le_bytes());
        assert_eq!(&b[28..32], &100.0f32.to_le_bytes());
        assert_eq!(&b[32..36], &0x10Fu32.to_le_bytes());
        assert_eq!(&b[36..40], &0x10u32.to_le_bytes());
        assert_eq!(&b[40..48], &[0u8; 8]);
        assert_eq!(Ray::from_bytes(&b), ray);
    }

    #[test]
    fn a_hit_is_eight_words_and_a_miss_is_all_zero() {
        let hit = Hit {
            t: 2.5,
            u: 0.25,
            v: 0.5,
            kind: hit_kind::BACK,
            instance_index: 3,
            instance_id: 0xAB_CDEF,
            geometry_index: 1,
            primitive_index: 77,
        };
        let b = hit.to_bytes();
        assert_eq!(b.len(), Hit::SIZE);
        assert_eq!(&b[0..4], &2.5f32.to_le_bytes());
        assert_eq!(&b[12..16], &2u32.to_le_bytes());
        assert_eq!(&b[20..24], &0xAB_CDEFu32.to_le_bytes());
        assert_eq!(&b[28..32], &77u32.to_le_bytes());
        assert_eq!(Hit::from_bytes(&b), hit);
        assert_eq!(Hit::MISS.to_bytes(), [0u8; 32]);
    }

    #[test]
    fn rays_d3d12_forbids_are_refused() {
        let ok = Ray::new([0.0; 3], [0.0, 0.0, 1.0], 0.0, f32::INFINITY);
        assert!(ok.validate().is_ok());
        let bad = |f: &dyn Fn(&mut Ray)| {
            let mut r = ok;
            f(&mut r);
            r.validate().is_err()
        };
        assert!(bad(&|r| r.dir = [0.0; 3]));
        assert!(bad(&|r| r.origin[1] = f32::NAN));
        assert!(bad(&|r| r.dir[2] = f32::INFINITY));
        assert!(bad(&|r| r.tmin = -1.0));
        assert!(bad(&|r| r.tmax = f32::NAN));
        assert!(bad(&|r| (r.tmin, r.tmax) = (2.0, 1.0)));
        assert!(bad(&|r| r.flags = 0x400));
        assert!(bad(
            &|r| r.flags = ray_flag::FORCE_OPAQUE | ray_flag::CULL_NON_OPAQUE
        ));
        assert!(bad(&|r| r.flags = 0x30));
        assert!(bad(&|r| r.flags = 0x300));
        assert!(bad(&|r| r.flags = 0x110));
    }
}
