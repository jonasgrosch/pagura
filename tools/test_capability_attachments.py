"""Public adapter contract controls with stubs; no local PDF engines imported."""
import importlib.util
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from capabilities import ROOT


def adapter(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "benchmarks/adapters" / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bindings = adapter("capability_python")
poppler = adapter("capability_poppler")
PAYLOAD = b"Synthetic attachment.\n"
EXPECTED = {"name": "probe.txt", "sha256": "bb0530979076ce983311c32a3cbf23448fb488520e3af66529a30246dbf01680"}


class AttachmentAdapterTests(unittest.TestCase):
    def test_pdfium_filters_subtypes_preserves_duplicates_and_closes_borrowed_handles(self):
        page = Mock()
        raw = SimpleNamespace(FPDFPage_GetAnnotCount=Mock(return_value=3),
                              FPDFPage_GetAnnot=Mock(side_effect=[10, 11, 12]),
                              FPDFAnnot_GetSubtype=Mock(side_effect=[2, 17, 17]),
                              FPDFAnnot_GetFileAttachment=Mock(return_value=20),
                              FPDFPage_CloseAnnot=Mock(), FPDF_ANNOT_FILEATTACHMENT=17)
        attachment = SimpleNamespace(get_name=lambda: "probe.txt", get_data=lambda: PAYLOAD)
        backend = SimpleNamespace(raw=raw, PdfAttachment=Mock(return_value=attachment))
        self.assertEqual(bindings.pdfium_annotations([page], backend), [EXPECTED, EXPECTED])
        self.assertEqual([call.args for call in raw.FPDFPage_CloseAnnot.call_args_list], [(10,), (11,), (12,)])
        self.assertEqual([call.args for call in raw.FPDFAnnot_GetFileAttachment.call_args_list], [(11,), (12,)])
        page.close.assert_called_once_with()

    def test_pdfium_decode_failure_still_closes_annotation_before_page(self):
        closed = []
        page = SimpleNamespace(close=lambda: closed.append("page"))
        raw = SimpleNamespace(FPDFPage_GetAnnotCount=lambda p: 1, FPDFPage_GetAnnot=lambda p, i: 10,
                              FPDFAnnot_GetSubtype=lambda a: 17, FPDFAnnot_GetFileAttachment=lambda a: 20,
                              FPDFPage_CloseAnnot=lambda a: closed.append("annotation"), FPDF_ANNOT_FILEATTACHMENT=17)
        attachment = SimpleNamespace(get_name=lambda: "probe.txt", get_data=Mock(side_effect=ValueError("decode failed")))
        backend = SimpleNamespace(raw=raw, PdfAttachment=lambda a, d: attachment)
        with self.assertRaisesRegex(ValueError, "decode failed"):
            bindings.pdfium_annotations([page], backend)
        self.assertEqual(closed, ["annotation", "page"])
        closed.clear()
        with patch.object(raw, "FPDFPage_GetAnnotCount", return_value=1025), self.assertRaises(bindings.ProbeLimit):
            bindings.pdfium_annotations([page], backend)
        self.assertEqual(closed, ["page"])

    def test_mupdf_uses_first_page_and_file_annotations_not_document_embedded_files(self):
        page = SimpleNamespace(annots=Mock(return_value=[SimpleNamespace(
            file_info={"filename": "legacy.txt", "ufilename": "probe.txt"}, get_file=lambda: PAYLOAD)]))
        result = bindings.mupdf_annotations([page], SimpleNamespace(PDF_ANNOT_FILE_ATTACHMENT=17))
        self.assertEqual(result, [EXPECTED])
        page.annots.assert_called_once_with(types=[17])
        with patch.object(bindings, "MAX_PAYLOAD", 2), self.assertRaises(bindings.ProbeLimit):
            bindings.mupdf_annotations([page], SimpleNamespace(PDF_ANNOT_FILE_ATTACHMENT=17))

    def test_poppler_uses_mapping_type_and_writes_only_a_fixed_temporary_path(self):
        saved = []
        def save(path):
            destination = Path(path)
            self.assertEqual(destination.name, "attachment.bin")
            self.assertFalse(destination.exists())
            destination.write_bytes(PAYLOAD)
            saved.append(destination)
            return True
        attachment = SimpleNamespace(save=save, get_name=lambda: "probe.txt")
        link = SimpleNamespace(get_annot_type=lambda: 2, get_attachment=Mock(side_effect=AssertionError("link selected")))
        file = SimpleNamespace(get_annot_type=lambda: 17, get_attachment=lambda: attachment)
        page = SimpleNamespace(get_annot_mapping=lambda: [SimpleNamespace(annot=a) for a in (link, file, file)])
        document = SimpleNamespace(get_page=Mock(return_value=page))
        backend = SimpleNamespace(AnnotType=SimpleNamespace(FILE_ATTACHMENT=17))
        self.assertEqual(poppler.read(document, "annotation-attachments", backend), [EXPECTED, EXPECTED])
        document.get_page.assert_called_once_with(0)
        self.assertTrue(all(not path.exists() for path in saved))
        with patch.object(page, "get_annot_mapping", return_value=[None] * 1025), self.assertRaises(poppler.ProbeLimit):
            poppler.read(document, "annotation-attachments", backend)


if __name__ == "__main__":
    unittest.main()
