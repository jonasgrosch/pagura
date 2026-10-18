"""Exact bounded-normalized RGB differences, with an optional fast backend."""


def pixel_difference(first, second):
    # The caller has already validated dimensions/payload budgets with normalize.
    from PIL import Image, ImageChops
    if first[:2] != second[:2]:
        return {"status": "dimension-mismatch", "first_dimensions": first[:2], "second_dimensions": second[:2]}
    histogram = ImageChops.difference(Image.frombytes("RGB", first[:2], first[2]),
                                     Image.frombytes("RGB", second[:2], second[2])).histogram()
    sums, maxima, different = [], [], 0
    for channel in range(3):
        counts = histogram[channel * 256:(channel + 1) * 256]
        sums.append(sum(value * count for value, count in enumerate(counts)))
        maxima.append(max(value for value, count in enumerate(counts) if count))
        different += sum(counts[1:])
    pixels = first[0] * first[1]
    return {"status": "comparable", "different_channels": different,
            "mean_channel_error": sum(sums) / (pixels * 3), "max_channel_error": max(maxima),
            "channel_mean_errors": [value / pixels for value in sums], "channel_max_errors": maxima}
