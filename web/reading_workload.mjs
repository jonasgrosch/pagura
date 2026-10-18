/** Independent synthetic PDF for reading measurements. No Pagura writer is used.
 * Type 3 cells have fixed advances; ToUnicode maps printable ASCII identically.
 */
export function readingWorkload(pages = 100, lines = 50) {
  const line = 'Pagura reader search selection copy navigation. '.repeat(2).slice(0, 90);
  const content = `BT /F 10 Tf 1 0 0 1 20 760 Tm 14 TL\n${Array.from({ length: lines }, () => `(${line}) Tj T*`).join('\n')}\nET\n`;
  const font = pages + 3,
    glyph = pages + 4,
    space = pages + 5,
    stream = pages + 6,
    cmap = pages + 7;
  const streamObject = (data) => `<< /Length ${new TextEncoder().encode(data).length} >>\nstream\n${data}\nendstream`;
  const objects = [
    null,
    '<< /Type /Catalog /Pages 2 0 R >>',
    `<< /Type /Pages /Count ${pages} /Kids [${Array.from({ length: pages }, (_, i) => `${i + 3} 0 R`).join(' ')}] >>`,
  ];
  for (let i = 0; i < pages; i++)
    objects.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F ${font} 0 R >> >> /Contents ${stream} 0 R >>`,
    );
  const codes = Array.from({ length: 95 }, (_, i) => i + 32);
  objects.push(
    `<< /Type /Font /Subtype /Type3 /Name /F /FontBBox [0 0 600 1000] /FontMatrix [0.001 0 0 0.001 0 0] /Resources << >> /FirstChar 32 /LastChar 126 /Widths [${codes.map(() => 600).join(' ')}] /Encoding << /Type /Encoding /Differences [32 ${codes.map((c) => `/g${c}`).join(' ')}] >> /CharProcs << ${codes.map((c) => `/g${c} ${c === 32 ? space : glyph} 0 R`).join(' ')} >> /ToUnicode ${cmap} 0 R >>`,
  );
  objects.push(streamObject('600 0 0 0 600 1000 d1\n0 0 600 1000 re f\n'));
  objects.push(streamObject('600 0 0 0 0 0 d1\n'));
  objects.push(streamObject(content));
  objects.push(
    streamObject(
      `1 begincodespacerange <00> <ff> endcodespacerange\n95 beginbfchar\n${codes.map((c) => `<${c.toString(16)}> <${c.toString(16).padStart(4, '0')}>`).join('\n')}\nendbfchar\n`,
    ),
  );
  let pdf = '%PDF-1.7\n',
    offsets = [0];
  for (let i = 1; i < objects.length; i++) {
    offsets.push(pdf.length);
    pdf += `${i} 0 obj\n${objects[i]}\nendobj\n`;
  }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length}\n0000000000 65535 f \n${offsets
    .slice(1)
    .map((n) => `${String(n).padStart(10, '0')} 00000 n \n`)
    .join('')}trailer\n<< /Size ${objects.length} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return {
    bytes: new TextEncoder().encode(pdf),
    line,
    pages,
    lines,
    text: Array.from({ length: lines }, () => line.trimEnd()).join('\n'),
  };
}
