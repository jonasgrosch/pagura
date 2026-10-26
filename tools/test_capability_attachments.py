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
    def test_mupdf_catalog_uses_engine_name_tree_loader_and_exact_native_file_handles(self):
        files = [{"UF": "probe.txt", "F": "legacy.bin"}, {"UF": "", "F": "ignored.bin"}, {"F": "fallback.txt"}]
        document = object()
        raw = SimpleNamespace(
            pdf_open_document=Mock(return_value=document), pdf_new_name=Mock(return_value="EmbeddedFiles-name"),
            pdf_load_name_tree=Mock(return_value=files), pdf_dict_len=len,
            pdf_dict_get_val=lambda tree, index: tree[index], pdf_is_embedded_file=Mock(return_value=True),
            pdf_dict_gets=lambda file, key: file.get(key), pdf_is_null=lambda item: item is None,
            pdf_to_text_string=lambda item: item, pdf_load_embedded_file_contents=Mock(return_value=PAYLOAD),
            fz_buffer_extract=lambda item: item)
        result = bindings.mupdf_catalog(raw, "synthetic.pdf")
        raw.pdf_open_document.assert_called_once_with("synthetic.pdf")
        raw.pdf_load_name_tree.assert_called_once_with(document, "EmbeddedFiles-name")
        self.assertEqual(result, {"status": "ok", "value": [
            {"name": "", "sha256": EXPECTED["sha256"]},
            {"name": "fallback.txt", "sha256": EXPECTED["sha256"]}, EXPECTED]})
        self.assertEqual([call.args[0] for call in raw.pdf_load_embedded_file_contents.call_args_list], files)
        # The catalog operation cannot silently return the high-level binding's
        # empty list or fall back to page annotation enumeration.
        backend = SimpleNamespace(mupdf=raw, open=Mock(side_effect=AssertionError("wrong API")))
        with patch.dict("sys.modules", {"pymupdf": backend}):
            self.assertEqual(bindings.probe("mupdf", "attachments", "synthetic.pdf"), result)
        backend.open.assert_not_called()
        raw.pdf_dict_len = lambda tree: 0
        self.assertEqual(bindings.mupdf_catalog(raw, "empty.pdf"), {"status": "ok", "value": []})

    def test_mupdf_native_catalog_preserves_failure_and_resource_boundaries(self):
        raw = SimpleNamespace(pdf_open_document=Mock(side_effect=ValueError("private path")))
        self.assertEqual(bindings.mupdf_catalog(raw, "invalid.pdf"), {"status": "rejected", "reason": "ValueError"})
        self.assertEqual(bindings.mupdf_catalog(SimpleNamespace(), "source.pdf")["status"], "adapter-unavailable")
        raw.pdf_open_document = Mock(return_value=object())
        self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf")["status"], "adapter-unavailable")
        raw.pdf_new_name = lambda name: name
        raw.pdf_load_name_tree = lambda document, which: []
        raw.pdf_dict_len = lambda tree: 1025
        self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf")["status"], "limit")
        raw.pdf_dict_len = lambda tree: -1
        self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf"), {"status": "adapter-error", "reason": "ValueError"})
        raw.pdf_dict_len = lambda tree: 1
        raw.pdf_dict_get_val = lambda tree, index: "spec"
        raw.pdf_is_embedded_file = lambda file: False
        self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf")["status"], "adapter-error")
        raw.pdf_is_embedded_file = lambda file: True
        raw.pdf_dict_gets = lambda file, key: "probe.txt"
        raw.pdf_is_null = lambda item: False
        raw.pdf_to_text_string = lambda item: item
        raw.pdf_load_embedded_file_contents = lambda file: b"bytes"
        raw.fz_buffer_extract = lambda item: item
        with patch.object(bindings, "MAX_PAYLOAD", 4):
            self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf")["status"], "limit")
        raw.pdf_load_embedded_file_contents = Mock(side_effect=RuntimeError("private payload"))
        self.assertEqual(bindings.mupdf_catalog(raw, "source.pdf"), {"status": "adapter-error", "reason": "RuntimeError"})

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
