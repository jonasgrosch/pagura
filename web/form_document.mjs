/** Small independently authored AcroForm for the local editing demo and host tests. */
export function formDocument({ hiddenText = false, omitText = false, colourRepair = false, duplicateText = false, differentText = false, ambiguousText = false, missingFont = false, unknownFont = false, unmarked = false, emptyAppearance = false, needAppearances = false } = {}) {
  const stream = (text, extra = '') => `<< /Length ${text.length} ${extra} >>\nstream\n${text}endstream`;
  const appearance = '/Type /XObject /Subtype /Form /BBox [0 0 18 18] /Resources << >>';
  const objects = [null,
    '<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>',
    '<< /Type /Pages /Count 1 /Kids [3 0 R] >>',
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 440] /Resources << /Font << /Helv 5 0 R >> /ColorSpace << /Spot [/Separation /Spot /DeviceRGB 17 0 R] >> >> /Contents 6 0 R /Annots [${omitText ? '' : '7 0 R'} 8 0 R 9 0 R 10 0 R 11 0 R] >>`,
    `<< /Fields [${duplicateText ? 18 : 7} 0 R ${ambiguousText ? "19 0 R" : ""} 8 0 R 9 0 R 12 0 R] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >>`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    stream((colourRepair ? '/Spot cs 1 sc 0 0 10 10 re f\n' : '') + 'BT /Helv 22 Tf 40 392 Td (Pagura form editing) Tj ET\nBT /Helv 11 Tf 40 365 Td (Click a field to fill it, then choose Save PDF.) Tj ET\nBT /Helv 12 Tf 40 340 Td (Full name) Tj ET\nBT /Helv 12 Tf 40 270 Td (Team) Tj ET\nBT /Helv 12 Tf 68 154 Td (Subscribe to updates) Tj ET\nBT /Helv 12 Tf 40 132 Td (Preferred contact) Tj ET\nBT /Helv 12 Tf 68 104 Td (Email) Tj ET\nBT /Helv 12 Tf 208 104 Td (Phone) Tj ET\n0.7 G 40 300 340 28 re S 40 230 240 28 re S\n'),
    `<< /Type /Annot /Subtype /Widget /FT /Tx /T (Full name) /Rect [40 300 380 328] /P 3 0 R /F ${hiddenText ? 6 : 4} /V () /MaxLen 80 /AP << /N 15 0 R >> >>`,
    '<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (Team) /Rect [40 230 280 258] /P 3 0 R /F 4 /Opt [(Engineering) (Design) (Research)] /AP << /N 16 0 R >> >>',
    '<< /Type /Annot /Subtype /Widget /FT /Btn /T (Subscribe) /Rect [40 150 58 168] /P 3 0 R /F 4 /V /Off /AS /Off /AP << /N << /Off 13 0 R /Yes 14 0 R >> >> >>',
    '<< /Type /Annot /Subtype /Widget /Parent 12 0 R /Rect [40 100 58 118] /P 3 0 R /F 4 /AS /Off /AP << /N << /Off 13 0 R /Email 14 0 R >> >> >>',
    '<< /Type /Annot /Subtype /Widget /Parent 12 0 R /Rect [180 100 198 118] /P 3 0 R /F 4 /AS /Off /AP << /N << /Off 13 0 R /Phone 14 0 R >> >> >>',
    '<< /FT /Btn /Ff 32768 /T (Preferred contact) /Kids [10 0 R 11 0 R] /Opt [(Email) (Phone)] /V /Off >>',
    stream('0 G 1 w 1 1 16 16 re S\n', appearance),
    stream('0 G 1 w 1 1 16 16 re S 2 w 4 4 m 14 14 l S 4 14 m 14 4 l S\n', appearance),
    stream('', '/Type /XObject /Subtype /Form /BBox [0 0 340 28] /Resources << >>'),
    stream('', '/Type /XObject /Subtype /Form /BBox [0 0 240 28] /Resources << >>'),
    '<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 0] /N 1 >>',
  ];
  if (needAppearances) objects[4] = objects[4].replace('/Fields', '/NeedAppearances true /Fields');
  if (missingFont) objects[4] = objects[4].replace('/Helv 12 Tf', `/${unknownFont ? 'CustomFont' : 'Helvetica'} 12 Tf`).replace('/DR << /Font << /Helv 5 0 R >> >>', '');
  if (unmarked) {
    objects[7] = objects[7].replace('/V ()', '/V (Previous value)');
    objects[15] = stream('q 0.8 G 1 1 338 26 re S Q\nBT /Helv 12 Tf 2 8 Td (Previous value) Tj ET\n', '/Type /XObject /Subtype /Form /BBox [0 0 340 28] /Resources << /Font << /Helv 5 0 R >> >>');
  }
  if (emptyAppearance) objects[15] = stream('q Q\n', '/Type /XObject /Subtype /Form /BBox [0 0 0 0] /Resources << >>');
  if (duplicateText) objects.push(differentText ? objects[7].replace('/MaxLen 80', '/MaxLen 81') : objects[7]);
  if (ambiguousText) objects.push(objects[7]);
  let pdf = '%PDF-1.7\n'; const offsets = [0];
  for (let i = 1; i < objects.length; i++) { offsets.push(pdf.length); pdf += `${i} 0 obj\n${objects[i]}\nendobj\n`; }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length}\n0000000000 65535 f \n${offsets.slice(1).map((n) => `${String(n).padStart(10, '0')} 00000 n \n`).join('')}trailer\n<< /Root 1 0 R /Size ${objects.length} >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(pdf);
}
