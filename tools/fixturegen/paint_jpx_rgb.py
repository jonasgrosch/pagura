"""Matched ordinary RGB controls for the G01/R02 JPX pixel observations.

The independent primary-formula colours become uncompressed eight-bit RGB
samples; opacity, when present, becomes a separate ordinary soft mask. Neither
JPX decoding nor a measured renderer supplies any sample. Native and twofold
placements retain the original oracle and tolerance. These controls distinguish
ordinary image sampling/colour/soft-mask behaviour from the combined JPX path;
they do not adjudicate a disagreement by themselves.
"""
from dataclasses import replace

from . import paint_jpx
from .model import RENDER
from .paint import image_case
from .pdf import stream

AREA = "paint_jpx_rgb"


def rgb_image(name, encoded, dictionary, samples, width, height, *, scale=2, **options):
    colors, alpha = options['colors'], options.get('alpha')
    raw = bytes(channel for color in colors for channel in color)
    body = b'/ColorSpace /DeviceRGB /BitsPerComponent 8'
    masks = []
    if alpha is not None:
        values = [value * 255 for value in alpha]
        if any(value != int(value) for value in values):
            raise ValueError('control opacity must have an exact eight-bit representation')
        mask = b'/Type /XObject /Subtype /Image /ColorSpace /DeviceGray /BitsPerComponent 8 /Width %d /Height %d' % (width, height)
        masks.append(stream(bytes(int(value) for value in values), mask))
        body += b' /SMask 6 0 R'
    source_area = 'paint_jpx_native' if scale == 1 else 'paint_jpx'
    control = image_case(name + ('-native' if scale == 1 else ''), raw, body, [], width, height,
                         matrix=(width * scale, 0, 0, height * scale, 2, 2), masks=masks,
                         colors=colors, alpha=alpha, requirement=options['requirement'],
                         tolerance=options.get('tolerance', 0))
    return replace(control, description=f'Uncompressed RGB/separate-soft-mask control for {source_area}/{name}',
                   extra={**control.extra, 'control_of': f'{source_area}/{name}'})


def native_rgb(name, encoded, dictionary, samples, width, height, **options):
    return rgb_image(name, encoded, dictionary, samples, width, height, scale=1, **options)


def cases():
    for paint in (rgb_image, native_rgb):
        for case in paint_jpx.cases(paint=paint):
            if case.expected == RENDER:
                yield case
