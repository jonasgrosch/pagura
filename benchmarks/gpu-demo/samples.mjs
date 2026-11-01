/** Independently authored PDF workloads, generated locally without libraries. */
const encoder = new TextEncoder();
export function pdf(content, extra = []) {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F 5 0 R >> /XObject << /Im 6 0 R >> /ExtGState << /A 6 0 R >> >> /Contents 4 0 R >>',
    `<< /Length ${encoder.encode(content).length} >>\nstream\n${content}\nendstream`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>', ...extra,
  ];
  let text = '%PDF-1.7\n', offsets = [0];
  for (let i = 0; i < objects.length; i++) { offsets.push(encoder.encode(text).length); text += `${i + 1} 0 obj\n${objects[i]}\nendobj\n`; }
  const xref = encoder.encode(text).length;
  text += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  text += offsets.slice(1).map(offset => `${String(offset).padStart(10, '0')} 00000 n \n`).join('');
  text += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return encoder.encode(text);
}

function heading(title, subtitle) {
  return `0.06 0.13 0.21 rg 0 0 612 792 re f\n0.25 0.84 0.67 rg 36 724 540 3 re f\n1 1 1 rg BT /F 25 Tf 36 749 Td (${title}) Tj ET\n0.6 0.7 0.8 rg BT /F 10 Tf 36 705 Td (${subtitle}) Tj ET\n`;
}

export const samples = [
  {id: 'text', label: 'Text page', make() {
    let content = heading('Prepared once. Painted exactly.', 'Dense outline text, clipped on the exact device grid.');
    for (let i = 0; i < 47; i++) content += `${i % 5 === 0 ? '0.25 0.84 0.67' : '0.9 0.94 0.97'} rg BT /F 10 Tf 36 ${680 - i * 13} Td (${String(i + 1).padStart(2, '0')}  The scene keeps paths and glyphs; the host chooses execution.) Tj ET\n`;
    return pdf(content);
  }},
  {id: 'vectors', label: 'Vector chart', make() {
    let content = heading('A thousand small decisions.', 'Strokes, fractional edges and geometric clipping.');
    content += 'q 35.25 55.5 541.5 625.25 re W n\n';
    for (let y = 0; y < 24; y++) for (let x = 0; x < 22; x++) {
      const px = 37 + x * 25.2, py = 60 + y * 25.3;
      content += `${0.12 + x / 28} ${0.25 + y / 35} 0.68 rg ${px} ${py} 20.7 20.4 re f\n`;
      content += `0.8 0.97 0.92 RG 0.6 w ${px} ${py} m ${px + 19.6} ${py + 18.8} l S\n`;
    }
    return pdf(content + 'Q\n');
  }},
  {id: 'overlap', label: 'Overlapping fills', make() {
    let content = heading('Coverage meets composition.', 'Seventy overlapping opaque fills with antialiased edges.');
    for (let i = 0; i < 70; i++) {
      const inset = i * 3.4;
      content += `${0.1 + i / 100} ${0.45 + i / 200} ${0.7 - i / 150} rg ${36 + inset} ${60 + inset} ${540 - inset * 2} ${620 - inset * 2} re f\n`;
    }
    return pdf(content);
  }},
  {id: 'image', label: 'Image (CPU fallback)', make() {
    const image = 'ff404040ffc04070ffffe020';
    return pdf(heading('Same page. Explicit fallback.', 'Sampled images keep their exact CPU renderer in this experiment.') + 'q 540 0 0 580 36 80 cm /Im Do Q\n',
      [`<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length ${image.length + 1} >>\nstream\n${image}>\nendstream`]);
  }},
  {id: 'alpha', label: 'Transparency (CPU fallback)', make() {
    return pdf(heading('Precision is a requirement.', 'Dynamic opacity is routed to the CPU without changing the rendering.')
      + '0.2 0.8 0.7 rg 40 100 380 450 re f q /A gs 0.9 0.3 0.6 rg 180 200 380 450 re f Q\n',
      ['<< /Type /ExtGState /ca 0.5 /CA 0.5 >>']);
  }},
];
