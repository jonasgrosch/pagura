//! SP 800-186 sections 3.2.1.3 (P-256), 3.2.1.4 (P-384),
//! and 3.2.1.5 (P-521). Each curve has a=-3 and cofactor one.

use super::{Point, Uint, field::Modulus};

#[derive(Debug)]
pub(crate) struct Parameters {
    pub field: Modulus,
    pub scalar: Modulus,
    pub b: Uint,
    pub gx: Uint,
    pub gy: Uint,
}

impl Parameters {
    pub fn generator(&self) -> Point {
        Point::affine(self.field.encode(self.gx), self.field.encode(self.gy), self)
    }

    pub fn equation(&self, x: Uint) -> Uint {
        let f = &self.field;
        let three_x = f.add(f.add(x, x), x);
        f.add(f.sub(f.mul(f.square(x), x), three_x), f.encode(self.b))
    }
}

pub(super) static P256: Parameters = Parameters {
    field: Modulus::new(
        Uint([
            0xffff_ffff_ffff_ffff,
            0x0000_0000_ffff_ffff,
            0,
            0xffff_ffff_0000_0001,
            0,
            0,
            0,
            0,
            0,
        ]),
        4,
    ),
    scalar: Modulus::new(
        Uint([
            0xf3b9_cac2_fc63_2551,
            0xbce6_faad_a717_9e84,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_0000_0000,
            0,
            0,
            0,
            0,
            0,
        ]),
        4,
    ),
    b: Uint([
        0x3bce_3c3e_27d2_604b,
        0x651d_06b0_cc53_b0f6,
        0xb3eb_bd55_7698_86bc,
        0x5ac6_35d8_aa3a_93e7,
        0,
        0,
        0,
        0,
        0,
    ]),
    gx: Uint([
        0xf4a1_3945_d898_c296,
        0x7703_7d81_2deb_33a0,
        0xf8bc_e6e5_63a4_40f2,
        0x6b17_d1f2_e12c_4247,
        0,
        0,
        0,
        0,
        0,
    ]),
    gy: Uint([
        0xcbb6_4068_37bf_51f5,
        0x2bce_3357_6b31_5ece,
        0x8ee7_eb4a_7c0f_9e16,
        0x4fe3_42e2_fe1a_7f9b,
        0,
        0,
        0,
        0,
        0,
    ]),
};

pub(super) static P384: Parameters = Parameters {
    field: Modulus::new(
        Uint([
            0x0000_0000_ffff_ffff,
            0xffff_ffff_0000_0000,
            0xffff_ffff_ffff_fffe,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0,
            0,
            0,
        ]),
        6,
    ),
    scalar: Modulus::new(
        Uint([
            0xecec_196a_ccc5_2973,
            0x581a_0db2_48b0_a77a,
            0xc763_4d81_f437_2ddf,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0,
            0,
            0,
        ]),
        6,
    ),
    b: Uint([
        0x2a85_c8ed_d3ec_2aef,
        0xc656_398d_8a2e_d19d,
        0x0314_088f_5013_875a,
        0x181d_9c6e_fe81_4112,
        0x988e_056b_e3f8_2d19,
        0xb331_2fa7_e23e_e7e4,
        0,
        0,
        0,
    ]),
    gx: Uint([
        0x3a54_5e38_7276_0ab7,
        0x5502_f25d_bf55_296c,
        0x59f7_41e0_8254_2a38,
        0x6e1d_3b62_8ba7_9b98,
        0x8eb1_c71e_f320_ad74,
        0xaa87_ca22_be8b_0537,
        0,
        0,
        0,
    ]),
    gy: Uint([
        0x7a43_1d7c_90ea_0e5f,
        0x0a60_b1ce_1d7e_819d,
        0xe9da_3113_b5f0_b8c0,
        0xf8f4_1dbd_289a_147c,
        0x5d9e_98bf_9292_dc29,
        0x3617_de4a_9626_2c6f,
        0,
        0,
        0,
    ]),
};

pub(super) static P521: Parameters = Parameters {
    field: Modulus::new(
        Uint([
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0x0000_0000_0000_01ff,
        ]),
        9,
    ),
    scalar: Modulus::new(
        Uint([
            0xbb6f_b71e_9138_6409,
            0x3bb5_c9b8_899c_47ae,
            0x7fcc_0148_f709_a5d0,
            0x5186_8783_bf2f_966b,
            0xffff_ffff_ffff_fffa,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0x0000_0000_0000_01ff,
        ]),
        9,
    ),
    b: Uint([
        0xef45_1fd4_6b50_3f00,
        0x3573_df88_3d2c_34f1,
        0x1652_c0bd_3bb1_bf07,
        0x5619_3951_ec7e_937b,
        0xb8b4_8991_8ef1_09e1,
        0xa2da_725b_99b3_15f3,
        0x929a_21a0_b685_40ee,
        0x953e_b961_8e1c_9a1f,
        0x0000_0000_0000_0051,
    ]),
    gx: Uint([
        0xf97e_7e31_c2e5_bd66,
        0x3348_b3c1_856a_429b,
        0xfe1d_c127_a2ff_a8de,
        0xa14b_5e77_efe7_5928,
        0xf828_af60_6b4d_3dba,
        0x9c64_8139_053f_b521,
        0x9e3e_cb66_2395_b442,
        0x858e_06b7_0404_e9cd,
        0x0000_0000_0000_00c6,
    ]),
    gy: Uint([
        0x88be_9476_9fd1_6650,
        0x353c_7086_a272_c240,
        0xc550_b901_3fad_0761,
        0x97ee_7299_5ef4_2640,
        0x17af_bd17_273e_662c,
        0x98f5_4449_579b_4468,
        0x5c8a_5fb4_2c7d_1bd9,
        0x3929_6a78_9a3b_c004,
        0x0000_0000_0000_0118,
    ]),
};
