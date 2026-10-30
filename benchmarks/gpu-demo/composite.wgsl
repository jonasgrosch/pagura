// Exact ADR 0004 N16 source-over. All arithmetic is u32, one invocation
// owns each pixel, and every bin retains the CPU span store order.
@group(0) @binding(0) var<storage, read> program: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;

fn unpack(rg: u32, ba: u32) -> vec4<u32> {
    return vec4<u32>(rg & 65535u, rg >> 16u, ba & 65535u, ba >> 16u);
}

fn cover(span: u32, x: u32) -> u32 {
    let start = program[span + 1u];
    if (x < start || x - start >= program[span + 2u]) { return 0u; }
    let offset = program[span + 3u];
    if (offset == 0xffffffffu) { return 255u; }
    let at = offset + x - start;
    return (program[program[8] + at / 4u] >> ((at % 4u) * 8u)) & 255u;
}

@compute @workgroup_size(64, 1, 1)
fn composite(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let width = program[2];
    if (x >= width || y >= program[3]) { return; }
    let bin = y * program[4] + x / 32u;
    let range = program[9] + bin * 2u;
    let first = program[range];
    let end = program[range + 1u];
    var pixel = unpack(program[5], program[6]);
    var resume = first;
    // ADR 0034 §4: a later opaque Normal store replaces every earlier
    // store at this pixel. Admission excludes masks, transfer and blends.
    // Find that store without changing the order of any remaining blend.
    for (var i = end; i > first;) {
        i--;
        let span = program[7] + program[program[10] + i] * 8u;
        if (cover(span, x) == 255u) {
            pixel = unpack(program[span + 4u], program[span + 5u]);
            resume = i + 1u;
            break;
        }
    }
    for (var i = resume; i < end; i++) {
        let id = program[program[10] + i];
        let span = program[7] + id * 8u;
        let c = cover(span, x);
        if (c == 0u) { continue; }
        let source = unpack(program[span + 4u], program[span + 5u]);
        // At most 255*65535+127: no lane can overflow u32.
        pixel = (source * c + pixel * (255u - c) + vec4<u32>(127u)) / 255u;
    }
    // round(word/257) is never a tie. No normalized texture conversion.
    let byte = (pixel + vec4<u32>(128u)) / 257u;
    output[y * width + x] = byte.x | (byte.y << 8u) | (byte.z << 16u) | (byte.w << 24u);
}
