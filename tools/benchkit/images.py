"""Bounded raw-image normalization and investigation-only pixel agreement."""


def ppm(data):
    """Read one 8-bit binary RGB PPM. Consume no bytes of its pixel payload as whitespace."""
    if len(data) > 64 * 1024 * 1024:
        raise ValueError("image exceeds budget")
    cursor, tokens = 0, []
    while len(tokens) < 4:
        while cursor < len(data):
            if data[cursor] in b" \t\r\n\v\f":
                cursor += 1
            elif data[cursor] == 35:
                end = data.find(b"\n", cursor)
                if end < 0:
                    raise ValueError("unterminated PPM comment")
                cursor = end + 1
            else:
                break
        start = cursor
        while cursor < len(data) and data[cursor] not in b" \t\r\n\v\f#":
            cursor += 1
        if cursor == start:
            raise ValueError("truncated PPM header")
        tokens.append(data[start:cursor])
    if tokens[0] != b"P6" or tokens[3] != b"255":
        raise ValueError("expected RGB8 P6")
    width, height = int(tokens[1]), int(tokens[2])
    if width <= 0 or height <= 0 or width * height > 16_000_000:
        raise ValueError("invalid PPM dimensions")
    if cursor >= len(data) or data[cursor] not in b" \t\r\n\v\f":
        raise ValueError("missing PPM separator")
    separator = 2 if data[cursor:cursor + 2] == b"\r\n" else 1
    pixels = data[cursor + separator:]
    if len(pixels) != width * height * 3:
        raise ValueError("PPM payload length mismatch")
    return width, height, pixels


def _dimensions(width, height):
    if width <= 0 or height <= 0 or width * height > 16_000_000:
        raise ValueError("invalid image dimensions")


def _rgba_rgb(pixels, premultiplied=False):
    """Composite RGBA8 on white with integer round-to-nearest for straight alpha."""
    rgb = bytearray(len(pixels) // 4 * 3)
    for index in range(0, len(pixels), 4):
        alpha = pixels[index + 3]
        for channel in range(3):
            value = pixels[index + channel]
            if premultiplied:
                if value > alpha:
                    raise ValueError("invalid premultiplied channel")
                value += 255 - alpha
            else:
                value = (value * alpha + 127) // 255 + 255 - alpha
            rgb[index // 4 * 3 + channel] = value
    return bytes(rgb)


def normalize(data):
    """Normalize bounded P6, RGB/RGB_ALPHA P7, or RGBA8 to RGB over white.

    RGBA8 is the adapter format: ASCII `RGBA8 WIDTH HEIGHT straight\n` (or
    `premultiplied`) followed by tightly packed, top-to-bottom RGBA bytes.
    PAM RGB_ALPHA is straight alpha. Pixel payloads are never whitespace-trimmed.
    """
    if len(data) > 64 * 1024 * 1024:
        raise ValueError("image exceeds budget")
    if data.startswith(b"P6"):
        return ppm(data)
    if data.startswith(b"RGBA8 "):
        header, separator, pixels = data.partition(b"\n")
        if len(header) > 4096:
            raise ValueError("oversized RGBA8 header")
        fields = header.split()
        if not separator or len(fields) != 4 or fields[3] not in (b"straight", b"premultiplied"):
            raise ValueError("invalid RGBA8 header")
        width, height = int(fields[1]), int(fields[2])
        _dimensions(width, height)
        if len(pixels) != width * height * 4:
            raise ValueError("RGBA8 payload length mismatch")
        return width, height, _rgba_rgb(pixels, fields[3] == b"premultiplied")
    if not data.startswith((b"P7\n", b"P7\r\n")):
        raise ValueError("unsupported raw image format")
    cursor, fields = data.index(b"\n") + 1, {}
    while True:
        end = data.find(b"\n", cursor, 4096)
        if end < 0:
            raise ValueError("truncated or oversized PAM header")
        line = data[cursor:end].split(b"#", 1)[0].strip()
        cursor = end + 1
        if line == b"ENDHDR":
            break
        if not line:
            continue
        entry = line.split()
        if len(entry) != 2 or entry[0] in fields:
            raise ValueError("invalid or duplicate PAM field")
        fields[entry[0]] = entry[1]
    if set(fields) != {b"WIDTH", b"HEIGHT", b"DEPTH", b"MAXVAL", b"TUPLTYPE"}:
        raise ValueError("missing or unknown PAM field")
    width, height, depth = (int(fields[key]) for key in (b"WIDTH", b"HEIGHT", b"DEPTH"))
    _dimensions(width, height)
    if fields[b"MAXVAL"] != b"255" or (depth, fields[b"TUPLTYPE"]) not in ((3, b"RGB"), (4, b"RGB_ALPHA")):
        raise ValueError("expected RGB8 or RGBA8 PAM")
    pixels = data[cursor:]
    if len(pixels) != width * height * depth:
        raise ValueError("PAM payload length mismatch")
    return width, height, pixels if depth == 3 else _rgba_rgb(pixels)


def pixel_difference(first, second):
    """Investigation data only: no resizing, alignment, tolerances or correctness claim."""
    if first[:2] != second[:2]:
        return {"status": "dimension-mismatch", "first_dimensions": first[:2], "second_dimensions": second[:2]}
    sums, maxima, different = [0, 0, 0], [0, 0, 0], 0
    for index, (a, b) in enumerate(zip(first[2], second[2])):
        value = abs(a - b)
        channel = index % 3
        sums[channel] += value
        maxima[channel] = max(maxima[channel], value)
        different += value != 0
    pixels = first[0] * first[1]
    return {"status": "comparable", "different_channels": different,
            "mean_channel_error": sum(sums) / (pixels * 3), "max_channel_error": max(maxima),
            "channel_mean_errors": [value / pixels for value in sums], "channel_max_errors": maxima}

