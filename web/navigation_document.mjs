/** Independent PDF geometry for host, DOM and real-browser navigation checks.
 * ISO 32000-2 7.5.4, 12.3.2.2, 12.3.3 and 12.5.6.5. */
export function navigationDocument() {
  const fits = ['FitB', 'FitBH', 'FitBV'];
  const destinations = Array.from({ length: 6 }, (_, i) => {
    const fit = fits[i % 3],
      page = i < 3 ? 4 : 5;
    return `[${page} 0 R /${fit}${fit === 'FitBH' ? ' 300' : fit === 'FitBV' ? ' 60' : ''}]`;
  });
  const stream = (text) => `<< /Length ${text.length} >>\nstream\n${text}endstream`;
  const objects = [
    null,
    '<< /Type /Catalog /Pages 2 0 R /Outlines 9 0 R /PageLabels << /Nums [0 << /P (Index) >> 1 << /P (Painted) >> 2 << /P (Empty) >>] >> >>',
    '<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>',
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 6 0 R /Annots [${destinations.map((_, i) => `${16 + i} 0 R`).join(' ')}] >>`,
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 7 0 R >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 8 0 R >>',
    stream('0.8 g 20 480 300 260 re f\n'),
    stream('1 0 0 rg 60 100 120 200 re f\n'),
    stream(''),
    '<< /Type /Outlines /Count 6 /First 10 0 R /Last 15 0 R >>',
  ];
  for (const [i, dest] of destinations.entries())
    objects.push(
      `<< /Title (${fits[i % 3]} ${i < 3 ? 'painted' : 'empty'}) /Parent 9 0 R ${i ? `/Prev ${9 + i} 0 R` : ''} ${i < 5 ? `/Next ${11 + i} 0 R` : ''} /Dest ${dest} >>`,
    );
  for (const [i, dest] of destinations.entries())
    objects.push(`<< /Type /Annot /Subtype /Link /Rect [20 ${700 - i * 40} 320 ${730 - i * 40}] /Dest ${dest} >>`);
  let pdf = '%PDF-2.0\n';
  const offsets = [0];
  for (let i = 1; i < objects.length; i++) {
    offsets.push(pdf.length);
    pdf += `${i} 0 obj\n${objects[i]}\nendobj\n`;
  }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length}\n0000000000 65535 f \n${offsets
    .slice(1)
    .map((n) => `${String(n).padStart(10, '0')} 00000 n \n`)
    .join('')}trailer\n<< /Root 1 0 R /Size ${objects.length} >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(pdf);
}
