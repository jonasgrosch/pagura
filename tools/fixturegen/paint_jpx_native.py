"""G01 matched JPX colour/opacity controls at one source sample per page pixel.

The R02 codestreams and primary-formula colours are unchanged. Only image
placement differs: at 72 DPI an integer-aligned width-by-height transform
avoids magnification. This supplies an additional sampling control, never a
replacement for the existing twofold-magnified fixtures or their tolerances.
"""
from . import paint_jpx
from .model import RENDER
from .paint import image_case

AREA = "paint_jpx_native"


def native_image(name, data, dictionary, samples, width, height, **options):
    return image_case(name, data, dictionary, samples, width, height,
                      matrix=(width, 0, 0, height, 2, 2), **options)


def cases():
    for case in paint_jpx.cases(paint=native_image):
        if case.expected == RENDER:
            yield case
