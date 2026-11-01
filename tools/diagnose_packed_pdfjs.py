"""Hosted-only four-case PDF.js diagnostic; capture completeness is not G01 success."""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
import signal
import subprocess
import threading
import time

CONFIG = {'source_commit': '70cc5196b66ad279ebe1bca4fc1369fa59440046', 'source_tree': '66a77281881099e250c6da2894aa8d15e73f04ab', 'source_files': {'.cargo/config.toml': '8e2a24365b3146e18cc4d583da8fdb00d68f8a55447afb176d54356ca924c7bf', '.github/workflows/capabilities.yml': 'bf4c4a5c295a4d92172c82078790bb5e055666337478b9c8009e89ee14238d1f', '.python-version': '2d82946ffb2ecf549699ebb86ecbbe663bbc19d53c49c681a1a425f003f80ddb', 'Cargo.lock': '50a08f96bed5436818924682a9703d648418d96395cc4345f4c9852b68ad1f06', 'Cargo.toml': '19a01a7c6948f54635b19b4257640c10652d57a055934cc1bbc9e31390439492', 'benchmarks/adapters/capability_pdfjs.mjs': '0762e9109208ec305ed9764150bc5c07f5cfe0f1fea8c4fb7dd2d0798ed49273', 'benchmarks/adapters/capability_pdfjs_services.mjs': '4d179076a62b4f3daa8d208badeea92a99465bac29c8c581996d1972ea17bba5', 'benchmarks/adapters/capability_poppler.py': '7ce62a2262a8c07515c5451e9e4a4a9990bdbcdc6d5757398ec3648b4ae3bbdc', 'benchmarks/adapters/capability_python.py': '535cf45c74c4bfe34944b0744554e784100dd10d6c2c26eaf756478f7757d39c', 'benchmarks/adapters/capability_quartz.swift': 'a5d1130920c783ec8b9d6cfa59c942d412dc7def71f22886ad6d3702cd180080', 'benchmarks/adapters/hayro/Cargo.lock': '3c7bd7c01cc8ab5887345ef07d35d69002016c32b6f36d9498178ca14b0af110', 'benchmarks/adapters/hayro/Cargo.toml': '5e69c1f5a3d6ea2bfb78701934787d3399edee2acc3f8b090a2d2f5a3d175231', 'benchmarks/adapters/hayro/src/bin/capabilities.rs': '2bd79dcd7311554034261300728bd9fb81c9c07400e5c7689ed6ddd9da216bdd', 'benchmarks/adapters/hayro/src/main.rs': 'ba44b146fe3a958013586883a8fec31ac0b67693056fe5abff0953942e980486', 'benchmarks/adapters/pdfjs.mjs': '34509f908a34b7e60a30fd913054be7c0415049c01d71371d4a16e5e08ff55a0', 'benchmarks/adapters/python_engine.py': '542c480fcdf968934103e3ae0560de340361897d87ba5680a3a8cdb6bdd1b587', 'benchmarks/adapters/quartz.swift': 'f57cefce4089b3f3690c60d960e5561814b4b1d0acc1db81c2b10e12c8277414', 'benchmarks/capabilities.json': 'f9287b00ad12f5b169e6488a4f39a58d589a65cb137110ac047869715d936ca5', 'benchmarks/node-package-lock.json': 'ded823aab917a80265b09674a882b567e55f26493e4d174f04438b489b38f42c', 'benchmarks/probes/rendering.json': 'f650ceb449eb624731fa8de9292cf73b498b931e66561667a16bfd3dc37380e1', 'corpus/manifests/classic.json': '9a917bcc6987be925908f5c40a5f217f480063731c7d805ebb0f9636b75374c9', 'corpus/manifests/packed_mask_tiles.json': '37a822379fbf91103514868e556e7e419dac0661d53165d8036c0aea987c1814', 'corpus/manifests/paint.json': '6979d9625ddf38fe5f57e566a61792adac61e81ae9f00b7957d4d38611ca88aa', 'crates/pagura-cli/Cargo.toml': '940a1c40ce4395c4f4b4e001d4afccadf3ab3e9ef3737a8d83feac83b9de750a', 'pyproject.toml': '510308c9855b184f67ef20ac3c7421fd761ec80210242713c745d58856a6c1ce', 'rust-toolchain.toml': '07647b6d1df86ed3a940cea32eb5c93a80d4d5e71926f849f500f15c2c2e3d5c', 'tools/benchkit/config.py': 'd8de933ac710f262e8e03708b199ff5180f3d003cdab9568a0790589cc052dd5', 'tools/benchkit/engines.py': '5abffec432c59423e148dc5eeffa785848ea4d52479b87ae0772dd2f15f6a195', 'tools/benchkit/images.py': '1978e8021ca1cf9ab07794306ea4aadb52cbee54d2ecd2c0dbca16fc2b1ef842', 'tools/capabilities.py': 'b9217bfa0401fd2af2c87e8d6ea0cf627d325e4d4716924563502aa9c82d7839', 'tools/capability_probes.py': '718f17bd452912be75f3b7207299ad38e58a995c2dee44b481ccd97e183994a2', 'tools/capability_render.py': 'df4cb64effdebe979872d1abbb4abe3febcf93c065dafc572c137e266b36b58e', 'tools/competitors.py': '9b625d6ab4f1d31b469dd1a9aba43c77f1016c2ce21c5006eb662f68201ff830', 'uv.lock': 'd82e08d0cd119622a13e95ddd099ea8c49874bfaa984b6d5fe7749e65e20c923', 'corpus/generated/classic/blank.pdf': '0a008af87ef7ca5f9df0e8278cf30133b5ef91909a24be0eef8a95d32286c687', 'corpus/generated/classic/blank.ppm': 'd2f6d1ef302851a667adde508208fded2aea933d1c0b62941b9d0882a3461523', 'corpus/generated/classic/rectangle.pdf': 'e787ff94fad3d42695b926c8b55e341961ea2adc0cb16158804b549b4270a9fe', 'corpus/generated/classic/rectangle.ppm': '5cb1259d260f10568506015a769a8c87a01cbebc7a701f46a576ac3d318a9b81', 'corpus/generated/classic/fractional-rectangle.pdf': '6fa4dd1ba6d90d8eaf4e8d62d07fae39469bf4a3de666a5059cff5b74fc80006', 'corpus/generated/classic/fractional-rectangle.ppm': 'f93bcc5b0ac5094959c49ad7127b33ca61f3738068262d2c0238e4e510294586', 'corpus/generated/paint/image-jpx.pdf': '4c6428c3beb6a361c762750b98be249fae255f253188fbd3726e8ce402b6393c', 'corpus/generated/paint/image-jpx.ppm': 'e8e83b0a70db650135319b8f67b9fd11cc33e2c0817eee36f39e7a03623fed67', 'corpus/generated/paint/image-jpx-alpha-1.pdf': '261cca460a2cc6297c1a1aa4b0e747d58a08cca5dd181ecc8122669e66974ef7', 'corpus/generated/paint/image-jpx-alpha-1.ppm': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'corpus/generated/paint/image-jpx-alpha-2.pdf': '1c084cb906adbfd043a274a71a4664f5043e57216ff62cf560624994e88712e5', 'corpus/generated/paint/image-jpx-alpha-2.ppm': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'corpus/generated/packed_mask_tiles/default-large.pdf': '4d14e3c4d34f47b6e81e6bacecc7cfc9c1eabd905ea2bf7a50567ac491d69a2c', 'corpus/generated/packed_mask_tiles/default-large.ppm': '85a5122d41b046abbd334ca3a2e102fd0fd9da9d4f34185f3fe665c9ae8296ff', 'corpus/generated/packed_mask_tiles/default-large-quarter-turn.pdf': 'ad28e572ab80c4e15232de1c198c6faef3c19e4403da7837de50c1bb970a4576', 'corpus/generated/packed_mask_tiles/default-large-quarter-turn.ppm': '0a2b716303c98e97539aec312a30c6ebfbae7ac89521e47f0da72b1d6461187a', 'corpus/generated/packed_mask_tiles/small-inverse.pdf': '58e7c2e1f46d5bbbc29fb8af181f2e1d817132c6cff89cf430c208b58757765a', 'corpus/generated/packed_mask_tiles/small-inverse.ppm': '3002cb72d3c3fa918e09f90fc082e421d9ffae7f3dd01bebd529323127d35b97', 'corpus/generated/packed_mask_tiles/minified-forty-rows.pdf': '5ecb695852f67b854baadeedd56c046769824c35fd23492f5c85e853602676fc', 'corpus/generated/packed_mask_tiles/minified-forty-rows.ppm': '8b11bb4af7dd2a47ff755706ea472cec5fecff081f3ec3256ffece5e0a658238', 'corpus/generated/packed_mask_tiles/minified-forty-mirror.pdf': '36d2b90a0574687e71d3269e94e205cce7a905e91a26e8ad987c429753789281', 'corpus/generated/packed_mask_tiles/minified-forty-mirror.ppm': 'a14864ce3c47e97ebfe5433f942c0d502a51b25cdab6fa9e827a88c6570a06c6', 'corpus/generated/packed_mask_tiles/minified-forty-256canvas.pdf': '33fbcdc69364bdde7a419a003ab213641f69c54db2648c6e2e28129852c91497', 'corpus/generated/packed_mask_tiles/minified-forty-256canvas.ppm': 'b2b3dc92f987cd2f22971cd5a76f1a5236fa8b1396871a12ec58e5e00fc1403c', 'corpus/generated/packed_mask_tiles/highwidth-short-crop.pdf': '64e22a2f4000047d977994a042d1b3cf1893e15a11d727e83ec767d84e461f1f', 'corpus/generated/packed_mask_tiles/highwidth-short-crop.ppm': 'f877abacf44a2687698db44ddb7f656e4a1b75fc8c01966ca408b077f9b1eac3', 'benchmarks/node-package.json': '840ad13cf7db36c03204654fb04b8a3484f9a90f93d172e94bc0c080593a230b'}, 'competitor_files': {'node/node_modules/@napi-rs/canvas-linux-x64-gnu/skia.linux-x64-gnu.node': 'b4af78b3eef20a5d30ca7a7544969bc812f35b9efdcccc4a7c6eb4465f7a3218', 'node/node_modules/pdfjs-dist/legacy/build/pdf.mjs': 'bc51f4874a66e3853189cb5d50aba4b3096afa83ed433b885d293a3d94583cdc', 'node/node_modules/pdfjs-dist/wasm/jbig2.wasm': '466f45c2a61a698152fb5400c27e56ff2ceb73dcb71fde3fc366a502308eaab0', 'node/node_modules/pdfjs-dist/wasm/openjpeg.wasm': '95e5002597af0824004aa57b1900fe019715db389e37640b1e58395e42f00cc5', 'node/node_modules/pdfjs-dist/wasm/qcms_bg.wasm': '663d86126d5f5fcb1c61490f94353e2a8375660b8c5498ab3ebab5a34b08800e', 'node/node_modules/pdfjs-dist/wasm/quickjs-eval.wasm': '7bcacc9f22cacf7e9b23866d2a6d1639693d40c7f144e41b7c69ed37ba9cbe8f', 'node/node_modules/pdfjs-dist/legacy/build/pdf.worker.mjs': 'bb422f60804477de4a0b09091d8347a916a387755ed1cbad24ccaec1d09c78bf', 'node/node_modules/pdfjs-dist/package.json': '8103b756525a79ec119558e9796062228290acfd5178a6b7fc93e6979511f604', 'node/node_modules/@napi-rs/canvas/index.js': 'bbdad32cab670261983ea56d4625fc7d07b1382f8bb7d6d31232bb2834861597', 'node/node_modules/@napi-rs/canvas/package.json': 'c189bbe7adf9ed5948d3ed82b8fd834b365c5d4adcb9bebe57e2ace3c54a3dff', 'node/package.json': '840ad13cf7db36c03204654fb04b8a3484f9a90f93d172e94bc0c080593a230b', 'node/package-lock.json': 'ded823aab917a80265b09674a882b567e55f26493e4d174f04438b489b38f42c'}, 'node_sha256': '19235a9b678f84729464c52623f92de130a165452747c6826d3fdc13df3abcc3', 'trace_adapter_sha256': '448b671c2f44bd5f3ef206c9e671b3da466398ed5bf1b227fb5de4164aa74e45', 'cases': [{'id': 'control.blank', 'dpi': 72, 'oracle': 'corpus/generated/classic/blank.ppm', 'oracle_sha256': 'd2f6d1ef302851a667adde508208fded2aea933d1c0b62941b9d0882a3461523', 'path': 'corpus/generated/classic/blank.pdf', 'sha256': '0a008af87ef7ca5f9df0e8278cf30133b5ef91909a24be0eef8a95d32286c687', 'tolerance': 0, 'expected': {'height': 64, 'rgb_sha256': '2a32d9a94209e87b46358ff2151efee07dea13d3171a3dfb4331dede6e060479', 'tolerance': 0, 'width': 64}}, {'id': 'control.rectangle', 'dpi': 72, 'oracle': 'corpus/generated/classic/rectangle.ppm', 'oracle_sha256': '5cb1259d260f10568506015a769a8c87a01cbebc7a701f46a576ac3d318a9b81', 'path': 'corpus/generated/classic/rectangle.pdf', 'sha256': 'e787ff94fad3d42695b926c8b55e341961ea2adc0cb16158804b549b4270a9fe', 'tolerance': 0, 'expected': {'height': 64, 'rgb_sha256': '9c1f47734a0f9c886b3819056c69774b65fb5d61fbb43ca5f849add25dd4f35c', 'tolerance': 0, 'width': 64}}, {'id': 'graphics.fractional', 'dpi': 72, 'oracle': 'corpus/generated/classic/fractional-rectangle.ppm', 'oracle_sha256': 'f93bcc5b0ac5094959c49ad7127b33ca61f3738068262d2c0238e4e510294586', 'path': 'corpus/generated/classic/fractional-rectangle.pdf', 'sha256': '6fa4dd1ba6d90d8eaf4e8d62d07fae39469bf4a3de666a5059cff5b74fc80006', 'tolerance': 1, 'expected': {'height': 64, 'rgb_sha256': '8cc43e71ada4b386f559040c1dcc042c81e935e8519127587e9f187a9d7658d1', 'tolerance': 1, 'width': 64}}, {'id': 'jpx.gray', 'dpi': 72, 'oracle': 'corpus/generated/paint/image-jpx.ppm', 'oracle_sha256': 'e8e83b0a70db650135319b8f67b9fd11cc33e2c0817eee36f39e7a03623fed67', 'path': 'corpus/generated/paint/image-jpx.pdf', 'sha256': '4c6428c3beb6a361c762750b98be249fae255f253188fbd3726e8ce402b6393c', 'tolerance': 0, 'expected': {'height': 32, 'rgb_sha256': 'b5cd30d0f4c249640527971a5f014c5a9fbb733ac02250f85bd623e15a5c892c', 'tolerance': 0, 'width': 32}}, {'id': 'jpx.alpha-straight', 'dpi': 72, 'oracle': 'corpus/generated/paint/image-jpx-alpha-1.ppm', 'oracle_sha256': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'path': 'corpus/generated/paint/image-jpx-alpha-1.pdf', 'sha256': '261cca460a2cc6297c1a1aa4b0e747d58a08cca5dd181ecc8122669e66974ef7', 'tolerance': 0, 'expected': {'height': 32, 'rgb_sha256': 'f6fda891c1973279318185b5d8fad2cd741b9b124e5205ba53b8ec3f08056fb8', 'tolerance': 0, 'width': 32}}, {'id': 'jpx.alpha-premultiplied', 'dpi': 72, 'oracle': 'corpus/generated/paint/image-jpx-alpha-2.ppm', 'oracle_sha256': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'path': 'corpus/generated/paint/image-jpx-alpha-2.pdf', 'sha256': '1c084cb906adbfd043a274a71a4664f5043e57216ff62cf560624994e88712e5', 'tolerance': 0, 'expected': {'height': 32, 'rgb_sha256': 'f6fda891c1973279318185b5d8fad2cd741b9b124e5205ba53b8ec3f08056fb8', 'tolerance': 0, 'width': 32}}, {'id': 'packed-mask.default', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/default-large.ppm', 'oracle_sha256': '85a5122d41b046abbd334ca3a2e102fd0fd9da9d4f34185f3fe665c9ae8296ff', 'path': 'corpus/generated/packed_mask_tiles/default-large.pdf', 'sha256': '4d14e3c4d34f47b6e81e6bacecc7cfc9c1eabd905ea2bf7a50567ac491d69a2c', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': 'dce72e8cf918d706235ffb434e7a9bb35fd3927032eec67d97317872d3c9473f', 'tolerance': 1, 'width': 8}}, {'id': 'packed-mask.quarter', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/default-large-quarter-turn.ppm', 'oracle_sha256': '0a2b716303c98e97539aec312a30c6ebfbae7ac89521e47f0da72b1d6461187a', 'path': 'corpus/generated/packed_mask_tiles/default-large-quarter-turn.pdf', 'sha256': 'ad28e572ab80c4e15232de1c198c6faef3c19e4403da7837de50c1bb970a4576', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': 'c98ef9326a7ea9a0e2bc206b2d214ca56f9894221cd042c991f3bcb89b155731', 'tolerance': 1, 'width': 8}}, {'id': 'packed-mask.inverse', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/small-inverse.ppm', 'oracle_sha256': '3002cb72d3c3fa918e09f90fc082e421d9ffae7f3dd01bebd529323127d35b97', 'path': 'corpus/generated/packed_mask_tiles/small-inverse.pdf', 'sha256': '58e7c2e1f46d5bbbc29fb8af181f2e1d817132c6cff89cf430c208b58757765a', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': 'aa030fd2c6ef285e5a3d3ead67ec06b3b76b1933831d06e12160b4e27ddf80ed', 'tolerance': 1, 'width': 8}}, {'id': 'packed-mask.minified40', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/minified-forty-rows.ppm', 'oracle_sha256': '8b11bb4af7dd2a47ff755706ea472cec5fecff081f3ec3256ffece5e0a658238', 'path': 'corpus/generated/packed_mask_tiles/minified-forty-rows.pdf', 'sha256': '5ecb695852f67b854baadeedd56c046769824c35fd23492f5c85e853602676fc', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': 'a8855ce6f79d1b7ebfac9e26412250806a01aac623c25683ddfa7f7e4d56b3a5', 'tolerance': 1, 'width': 144}}, {'id': 'packed-mask.mirror40', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/minified-forty-mirror.ppm', 'oracle_sha256': 'a14864ce3c47e97ebfe5433f942c0d502a51b25cdab6fa9e827a88c6570a06c6', 'path': 'corpus/generated/packed_mask_tiles/minified-forty-mirror.pdf', 'sha256': '36d2b90a0574687e71d3269e94e205cce7a905e91a26e8ad987c429753789281', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': '5f76e1078d6440096998b24955557acbff6e94205c4407099bf57792f69dd35b', 'tolerance': 1, 'width': 144}}, {'id': 'packed-mask.canvas256', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/minified-forty-256canvas.ppm', 'oracle_sha256': 'b2b3dc92f987cd2f22971cd5a76f1a5236fa8b1396871a12ec58e5e00fc1403c', 'path': 'corpus/generated/packed_mask_tiles/minified-forty-256canvas.pdf', 'sha256': '33fbcdc69364bdde7a419a003ab213641f69c54db2648c6e2e28129852c91497', 'tolerance': 1, 'expected': {'height': 256, 'rgb_sha256': '8b5a27160ca5814bf0e19dcd3f6d3acdd85d481a88e04a4884fa541cad48d78b', 'tolerance': 1, 'width': 256}}, {'id': 'packed-mask.highwidth', 'dpi': 72, 'oracle': 'corpus/generated/packed_mask_tiles/highwidth-short-crop.ppm', 'oracle_sha256': 'f877abacf44a2687698db44ddb7f656e4a1b75fc8c01966ca408b077f9b1eac3', 'path': 'corpus/generated/packed_mask_tiles/highwidth-short-crop.pdf', 'sha256': '64e22a2f4000047d977994a042d1b3cf1893e15a11d727e83ec767d84e461f1f', 'tolerance': 1, 'expected': {'height': 8, 'rgb_sha256': '9bbd8317eeb84e12c7d0765ef0459eff320d6acabe477f57862db1feba4dc550', 'tolerance': 1, 'width': 8}}], 'prior_actual_PPMs': {'control.blank': {'relative_path': 'pdfjs/0/actual.ppm', 'sha256': 'd2f6d1ef302851a667adde508208fded2aea933d1c0b62941b9d0882a3461523', 'rgb_sha256': '2a32d9a94209e87b46358ff2151efee07dea13d3171a3dfb4331dede6e060479', 'previous_outcome': 'pass'}, 'control.rectangle': {'relative_path': 'pdfjs/1/actual.ppm', 'sha256': '5cb1259d260f10568506015a769a8c87a01cbebc7a701f46a576ac3d318a9b81', 'rgb_sha256': '9c1f47734a0f9c886b3819056c69774b65fb5d61fbb43ca5f849add25dd4f35c', 'previous_outcome': 'pass'}, 'graphics.fractional': {'relative_path': 'pdfjs/2/actual.ppm', 'sha256': '3ed8b1b480e23ab62a7297faf71f16c58c34d0fa90900e770eead4520e07a980', 'rgb_sha256': 'dcb9b5f58271e9029a7f93a8b54b9f2322284de15e731df8974b54c257fef690', 'previous_outcome': 'pass'}, 'jpx.gray': {'relative_path': 'pdfjs/3/actual.ppm', 'sha256': 'e8e83b0a70db650135319b8f67b9fd11cc33e2c0817eee36f39e7a03623fed67', 'rgb_sha256': 'b5cd30d0f4c249640527971a5f014c5a9fbb733ac02250f85bd623e15a5c892c', 'previous_outcome': 'pass'}, 'jpx.alpha-straight': {'relative_path': 'pdfjs/4/actual.ppm', 'sha256': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'rgb_sha256': 'f6fda891c1973279318185b5d8fad2cd741b9b124e5205ba53b8ec3f08056fb8', 'previous_outcome': 'pass'}, 'jpx.alpha-premultiplied': {'relative_path': 'pdfjs/5/actual.ppm', 'sha256': 'd188e29f699a6d127485f045bb1f1161c1dad56b2e9f8be1ac5ea615f782d01b', 'rgb_sha256': 'f6fda891c1973279318185b5d8fad2cd741b9b124e5205ba53b8ec3f08056fb8', 'previous_outcome': 'pass'}, 'packed-mask.default': {'relative_path': 'pdfjs/6/actual.ppm', 'sha256': '4f18d06f1bf98c9eac9a4157a952a342f9ed7f60687c16724dd266653639877d', 'rgb_sha256': '562acb67a83ebcfb2d041572285362ed6ee4ecb1898b4423aa48754e725c4430', 'previous_outcome': 'incorrect'}, 'packed-mask.quarter': {'relative_path': 'pdfjs/7/actual.ppm', 'sha256': 'bf386f801cf2d71002b68a1aec5bbf6798ac81432e573bd7c9f321cf1eeeace7', 'rgb_sha256': 'c39091087a0e77db0288307d9d3e8337c2faa9d22b87106c02c6ef0ff08b9b00', 'previous_outcome': 'incorrect'}, 'packed-mask.inverse': {'relative_path': 'pdfjs/8/actual.ppm', 'sha256': 'f444abbea1859a9606143cee38cac212d507c49cca7dc77f9804cd7ce8455318', 'rgb_sha256': '6e0a0a28e6baf2de381de1cf81c66adfb9c667551be8596496e4329e27fa42c8', 'previous_outcome': 'incorrect'}}, 'cleanup_module_sha256': 'e42e641d1bb34f67de42d43ebbe3ede93c07ce1cf8a1b8eb360979dd6a095dc1', 'unit_module_sha256': '7d16df6714e50feb9c936efee4da92ae0c8552b1292885bb20bfdf92153285ef', 'prior_artifact_id': 13066831412, 'prior_run_id': 41263835098}
TRACE_ADAPTER = "// Independent benchmark glue written against Mozilla's documented Node API.\nimport fs from 'node:fs';\nimport { installCanvasCleanup } from './canvas-cleanup.mjs';\nimport os from 'node:os';\nimport path from 'node:path';\nimport { pathToFileURL, fileURLToPath } from 'node:url';\nimport { createRequire } from 'node:module';\nconst configured = process.env.PAGURA_COMPETITORS_DIR || path.join(os.homedir(), '.cache/pagura-competitors');\nconst cache = path.resolve(configured === '~' ? os.homedir() : configured.startsWith('~/') ? path.join(os.homedir(), configured.slice(2)) : configured);\nconst require = createRequire(path.join(cache, 'node/package.json'));\nconst modulePath = require.resolve('pdfjs-dist/legacy/build/pdf.mjs');\nconst pdfjs = await import(pathToFileURL(modulePath).href);\n// Diagnostic only: separate inherited FD 3, no renderer option/pixel/timeout change.\nconst trace = (stage, detail = {}) => fs.writeSync(3, JSON.stringify({stage, ...detail}) + '\\n');\ntrace('module-imported', {pdfjs: pdfjs.version, node: process.version,\n  canvas: require('@napi-rs/canvas/package.json').version});\n\nif (process.argv[2] === '--version') {\n  console.log(JSON.stringify({engine: pdfjs.version, node: process.version,\n    canvas: require('@napi-rs/canvas/package.json').version}));\n} else {\n  const args = process.argv.slice(2);\n  let password;\n  const passwordIndex = args.indexOf('--password');\n  if (passwordIndex >= 0) {\n    password = args[passwordIndex + 1];\n    args.splice(passwordIndex, 2);\n  }\n  if (![4, 5].includes(args.length)) throw new Error('expected INPUT OUTPUT DPI PAGE or INPUT OUTDIR DPI FIRST LAST');\n  const [source, target, dpi, first, last = first] = args;\n  if (!Number.isInteger(Number(first)) || !Number.isInteger(Number(last)) || Number(first) < 0 || Number(last) < Number(first)) {\n    throw new Error('invalid page range');\n  }\n  const started = process.hrtime.bigint();\n  const packageRoot = pathToFileURL(path.join(cache, 'node/node_modules/pdfjs-dist/'));\n  trace('getDocument-enter');\n  const loading = pdfjs.getDocument({\n    data: new Uint8Array(fs.readFileSync(source)), password,\n    cMapUrl: fileURLToPath(new URL('cmaps/', packageRoot)), cMapPacked: true,\n    standardFontDataUrl: fileURLToPath(new URL('standard_fonts/', packageRoot)),\n    wasmUrl: fileURLToPath(new URL('wasm/', packageRoot)), isEvalSupported: false,\n  });\n  trace('loading-promise-enter');\n  const document = await loading.promise;\n  installCanvasCleanup(document.canvasFactory);\n  trace('loading-promise-complete', {pages: document.numPages});\n  const factory = document.canvasFactory;\n  for (const method of ['create', 'reset', 'destroy']) {\n    const actual = factory[method];\n    factory[method] = function (...args) {\n      const dimensions = method === 'create' ? args :\n        method === 'reset' ? args.slice(1) : [args[0]?.canvas?.width, args[0]?.canvas?.height];\n      trace('canvas-' + method + '-enter', {dimensions});\n      try {\n        const result = Reflect.apply(actual, this, args);\n        trace('canvas-' + method + '-complete');\n        return result;\n      } catch (error) {\n        trace('canvas-' + method + '-throw', {name: error.name, message: error.message, stack: error.stack});\n        throw error;\n      }\n    };\n  }\n  const indices = Array.from({length: document.numPages}, (_, i) => i);\n  if (Number(last) >= indices.length) throw new Error('page range outside document');\n  const opened = process.hrtime.bigint();\n  const stages = [];\n  for (let index = Number(first); index <= Number(last); index++) {\n  const renderStarted = process.hrtime.bigint();\n  trace('getPage-enter', {index});\n  const page = await document.getPage(index + 1);\n  trace('getPage-complete', {index});\n  const viewport = page.getViewport({scale: Number(dpi) / 72});\n  const surface = document.canvasFactory.create(Math.ceil(viewport.width), Math.ceil(viewport.height));\n  trace('page-render-enter', {index});\n  await page.render({canvasContext: surface.context, viewport, background: 'rgb(255,255,255)'}).promise;\n  trace('page-render-complete', {index});\n  const width = surface.canvas.width, height = surface.canvas.height;\n  // Canvas 1.x can defer raster playback until readback. A one-pixel read\n  // flushes the complete recording here; full pixel extraction stays in emit.\n  trace('flush-enter');\n  surface.context.getImageData(0, 0, 1, 1);\n  trace('flush-complete');\n  const rendered = process.hrtime.bigint();\n  trace('full-readback-enter', {width, height});\n  const rgba = surface.context.getImageData(0, 0, width, height).data;\n  trace('full-readback-complete');\n  const output = args.length === 5 ? path.join(target, `page-${String(index + 1).padStart(4, '0')}.ppm`) : target;\n  fs.writeFileSync(output, Buffer.concat([Buffer.from(`RGBA8 ${width} ${height} straight\\n`), Buffer.from(rgba)]));\n  const emitted = process.hrtime.bigint();\n  stages.push({page: index, render_ns: Number(rendered - renderStarted), emit_ns: Number(emitted - rendered)});\n  document.canvasFactory.destroy(surface);\n  page.cleanup();\n  }\n  const total = process.hrtime.bigint();\n  console.log(JSON.stringify({schema: 1, open_ns: Number(opened - started), pages: stages,\n    total_ns: Number(total - started)}));\n  trace('loading-destroy-enter');\n  await loading.destroy();\n  trace('loading-destroy-complete');\n}\n"
CLEANUP_MODULE = '// Work around native zero-dimension fallbacks before the original release.\nexport function installCanvasCleanup(factory) {\n  const destroy = factory.destroy;\n  factory.destroy = function (entry) {\n    const canvas = entry?.canvas;\n    if (canvas) {\n      canvas.width = 1;\n      canvas.height = 1;\n    }\n    return Reflect.apply(destroy, this, [entry]);\n  };\n}\n'
UNIT_CONTROLS = "import assert from 'node:assert/strict';\nimport { createRequire } from 'node:module';\nimport path from 'node:path';\nimport { installCanvasCleanup } from './canvas-cleanup.mjs';\nconst require = createRequire(path.join(process.argv[2], 'node/package.json'));\nconst { createCanvas, ImageData } = require('@napi-rs/canvas');\nconst controls = [];\nclass LogicalCanvas {\n  constructor(width, height, limit = null) {\n    this.w = width; this.h = height; this.requests = []; this.limit = limit;\n  }\n  get width() { return this.w; }\n  get height() { return this.h; }\n  request(width, height) {\n    const area = BigInt(width) * BigInt(height);\n    this.requests.push([width, height, area.toString()]);\n    if (this.limit !== null && area > this.limit) throw new Error('model allocation refused');\n  }\n  set width(value) { const next = value <= 0 ? 350 : value; this.request(next, this.h); this.w = next; }\n  set height(value) { const next = value <= 0 ? 150 : value; this.request(this.w, next); this.h = next; }\n}\nfunction factory() {\n  const f = { calls: 0, receivers: [], missing: new Error('Canvas is not specified') };\n  f.destroy = function (entry) {\n    f.calls++; f.receivers.push(this);\n    const { canvas } = entry;\n    if (!canvas) throw f.missing;\n    canvas.width = canvas.height = 0;\n    entry.canvas = null; entry.context = null;\n  };\n  return f;\n}\nfor (const [width, height] of [[1, 1], [2, 3], [350, 150], [8, 8388608],\n  [8388608, 9], [34862, 4332], [3, 2], [524288, 1]]) {\n  const canvas = new LogicalCanvas(width, height);\n  const entry = { canvas, context: {} }; const f = factory();\n  installCanvasCleanup(f); f.destroy(entry);\n  const bound = BigInt(width) * BigInt(height) > 52500n ? BigInt(width) * BigInt(height) : 52500n;\n  assert(canvas.requests.every(([, , area]) => BigInt(area) <= bound));\n  assert.deepEqual([canvas.width, canvas.height], [350, 150]);\n  assert.equal(entry.canvas, null); assert.equal(entry.context, null);\n  assert.equal(f.calls, 1); assert.equal(f.receivers[0], f);\n  controls.push({ name: 'logical-request-bound', width, height, requests: canvas.requests, bound: bound.toString() });\n}\n{\n  const before = new LogicalCanvas(8388608, 9, 100000000n);\n  assert.throws(() => factory().destroy({ canvas: before, context: {} }), /model allocation refused/);\n  assert.deepEqual(before.requests[0], [8388608, 150, '1258291200']);\n  const after = new LogicalCanvas(8388608, 9, 100000000n); const f = factory();\n  installCanvasCleanup(f); f.destroy({ canvas: after, context: {} });\n  controls.push({ name: 'wide-before-fix-model-refusal', before: before.requests, after: after.requests });\n}\nfor (const entry of [null, undefined, { canvas: null, context: {} }]) {\n  const f = factory(); let before, after;\n  try { f.destroy(entry); } catch (error) { before = error; }\n  installCanvasCleanup(f);\n  try { f.destroy(entry); } catch (error) { after = error; }\n  assert(before && after); assert.equal(before.constructor, after.constructor);\n  assert.equal(before.message, after.message); assert.equal(f.calls, 2);\n  if (entry) assert(entry.context);\n  controls.push({ name: 'null-handle-error-preserved', error: after.name, message: after.message });\n}\n{\n  const f = factory(); const receiver = {}; installCanvasCleanup(f);\n  f.destroy.call(receiver, { canvas: new LogicalCanvas(5, 7), context: {} });\n  assert.equal(f.receivers[0], receiver);\n  controls.push({ name: 'actual-receiver-preserved' });\n}\n{\n  const sentinel = new Error('setter refusal'); const f = factory();\n  const entry = { canvas: { set width(_) { throw sentinel; } }, context: {} };\n  installCanvasCleanup(f); assert.throws(() => f.destroy(entry), (error) => error === sentinel);\n  assert.equal(f.calls, 0); assert(entry.canvas && entry.context);\n  controls.push({ name: 'setter-failure-not-swallowed' });\n}\nconst original = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255,\n  255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 0, 255];\nconst cyanRow = [0, 255, 255, 255, 0, 255, 255, 255, 0, 255, 255, 255];\nconst native = [];\nfor (const mutate of [false, true]) {\n  const source = createCanvas(3, 2); const ctx = source.getContext('2d');\n  ctx.putImageData(new ImageData(new Uint8ClampedArray(original), 3, 2), 0, 0);\n  const target = createCanvas(6, 2); const targetCtx = target.getContext('2d');\n  targetCtx.drawImage(source, 0, 0);\n  if (mutate) { ctx.fillStyle = 'rgb(0,255,255)'; ctx.fillRect(0, 0, 3, 2); }\n  targetCtx.drawImage(source, 3, 0);\n  const f = factory(); installCanvasCleanup(f);\n  const entry = { canvas: source, context: ctx }; f.destroy(entry);\n  const actual = Array.from(targetCtx.getImageData(0, 0, 6, 2).data);\n  const expected = [...original.slice(0, 12), ...(mutate ? cyanRow : original.slice(0, 12)),\n    ...original.slice(12), ...(mutate ? cyanRow : original.slice(12))];\n  native.push({ name: mutate ? 'native-copy-on-write-snapshot' : 'native-repeated-source-snapshot',\n    actual, expected, equal: actual.every((x, i) => x === expected[i]) });\n}\nconsole.log(JSON.stringify({ schema: 1, controls, native, pure_logical_controls: controls.length,\n  native_pixel_controls: native.length, passed: native.every((row) => row.equal),\n  scope: 'Logical request dimensions and real tiny native snapshots only; no giant allocation or OS bound claim.' }));\nassert(native.every((row) => row.equal));\n"


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def durable(path, value, append=False):
    with Path(path).open("ab" if append else "wb") as stream:
        stream.write((json.dumps(value, sort_keys=True) + "\n").encode())
        stream.flush()
        os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--prior-images", required=True, type=Path)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    temporary = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    output = args.output.absolute()
    prior_images = args.prior_images.resolve(strict=True)
    assert prior_images.is_relative_to(temporary)
    assert output.parent.resolve(strict=True) == temporary and not output.exists()
    assert os.environ.get("GITHUB_ACTIONS") == "true"
    assert os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
    assert os.environ.get("RUNNER_OS") == "Linux" and platform.system() == "Linux"
    assert platform.machine() == "x86_64"
    assert os.environ.get("GITHUB_REPOSITORY") == "jonasgrosch/pagura"
    assert os.environ.get("GITHUB_REF") == "refs/heads/codex/packed-mask-pdfjs-diagnostic-infra"
    assert digest(__file__) == os.environ["DIAGNOSTIC_RUNNER_SHA256"]
    infra = Path(__file__).resolve().parent.parent
    assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=infra, text=True).strip() == os.environ["GITHUB_SHA"]
    cache = Path(os.environ["PAGURA_COMPETITORS_DIR"]).resolve(strict=True)
    node_command = shutil.which("node")
    assert node_command is not None
    node = Path(node_command).resolve(strict=True)
    output.mkdir()
    state = {"schema": 1, "capture_complete": False, "G01_pass": False,
             "source_commit": CONFIG["source_commit"], "source_tree": CONFIG["source_tree"],
             "diagnostic_infra_commit": os.environ["GITHUB_SHA"],
             "workflow_run": os.environ["GITHUB_RUN_ID"], "source": str(source),
             "build_calls": 0, "render_processes_started": 0, "native_control_processes_started": 0,
             "all_child_processes_started": 0, "rows": [],
             "scope": "Cleanup-only validation: all13 immutable synthetic sources and20s render deadlines; one10s model/native-control process. "
                      "Timeouts and unknown errors remain incomplete G01 observations. "
                      "No source support, timing, accuracy admission, or native bound claim."}
    state["runner_sha256"] = digest(__file__)
    report_path = output / "report.json"

    def save():
        durable(report_path, state)

    def event(value):
        durable(output / "attempts.jsonl", value, append=True)

    pins = {}
    for rel, sha in CONFIG["source_files"].items():
        p = source / rel
        assert p.resolve(strict=True).is_relative_to(source)
        pins[str(p)] = sha
    for rel, sha in CONFIG["competitor_files"].items():
        p = cache / rel
        assert p.resolve(strict=True).is_relative_to(cache)
        pins[str(p)] = sha
    pins[str(node)] = CONFIG["node_sha256"]
    driver = output / "diagnostic-pdfjs.mjs"
    driver.write_text(TRACE_ADAPTER)
    assert digest(driver) == CONFIG["trace_adapter_sha256"]
    pins[str(driver)] = CONFIG["trace_adapter_sha256"]

    helper_path = output / "canvas-cleanup.mjs"
    helper_path.write_text(CLEANUP_MODULE)
    unit_path = output / "native-cleanup-controls.mjs"
    unit_path.write_text(UNIT_CONTROLS)
    pins[str(helper_path)] = CONFIG["cleanup_module_sha256"]
    pins[str(unit_path)] = CONFIG["unit_module_sha256"]
    for prior in CONFIG["prior_actual_PPMs"].values():
        p = prior_images / prior["relative_path"]
        assert p.resolve(strict=True).is_relative_to(prior_images)
        pins[str(p)] = prior["sha256"]

    def bookend():
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
        tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=source, text=True).strip()
        assert commit == CONFIG["source_commit"] and tree == CONFIG["source_tree"]
        for p, expected in pins.items():
            assert digest(p) == expected, p

    save()
    try:
        bookend()
        spec = importlib.util.spec_from_file_location("diagnostic_images", source / "tools/benchkit/images.py")
        assert spec is not None and spec.loader is not None
        images = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(images)
        state["verified_pins"] = pins
        planned = [{"id": "native-cleanup-controls", "role": "native-controls"}, *CONFIG["cases"]]
        for ordinal, case in enumerate(planned):
            bookend()
            assert shutil.disk_usage(output).free >= 3 * 1024**3
            folder = output / str(ordinal)
            folder.mkdir()
            target = folder / "page.ppm"
            is_control = case.get("role") == "native-controls"
            deadline = 10 if is_control else 20
            deadline_reason = str(deadline) + "-second-deadline"
            command = ([str(node), str(unit_path), str(cache)] if is_control else
                       [str(node), str(driver), str(source / case["path"]), str(target), "72", "0"])
            row = {"ordinal": ordinal, "case": case["id"], "command": command,
                   "role": "native-controls" if is_control else "pdfjs-render",
                   "input_sha256": case.get("sha256"), "deadline_seconds": deadline,
                   "state": "pending", "child_started": False}
            state["rows"].append(row)
            event(row)
            save()  # Durable intent before subprocess.Popen, including failures.
            child = None
            status = usage = None
            stop = threading.Event()
            reasons = []
            failure = None
            killed = []
            started = time.monotonic()
            with (folder / "trace.jsonl").open("xb") as trace:
                assert trace.fileno() == 3, "refuse unknown inherited FD3; never clobber it"
                with (folder / "stdout").open("xb") as out, (folder / "stderr").open("xb") as err:
                    watch = None
                    try:
                        child = subprocess.Popen(command, cwd=source, stdin=subprocess.DEVNULL,
                                                 stdout=out, stderr=err, pass_fds=(3,),
                                                 start_new_session=True)
                        row["child_started"] = True
                        row["pid"] = child.pid
                        state["all_child_processes_started"] += 1
                        state["native_control_processes_started" if is_control else "render_processes_started"] += 1
                        event({"state": "spawned", "ordinal": ordinal, "pid": child.pid})
                        save()

                        def watchdog():
                            while not stop.wait(0.025):
                                reason = (deadline_reason if time.monotonic() - started >= deadline else
                                          "diagnostic-log-budget" if
                                          max(os.fstat(out.fileno()).st_size, os.fstat(err.fileno()).st_size) > 65536
                                          or os.fstat(trace.fileno()).st_size > 131072 else None)
                                if reason is not None:
                                    reasons.append(reason)
                                    try:
                                        assert child is not None
                                        os.killpg(child.pid, signal.SIGKILL)
                                        killed.append(child.pid)
                                    except ProcessLookupError:
                                        pass
                                    return

                        watch = threading.Thread(target=watchdog, daemon=True)
                        watch.start()
                        _, status, usage = os.wait4(child.pid, 0)
                    except BaseException as exc:
                        failure = repr(exc)
                        if child:
                            try:
                                os.killpg(child.pid, signal.SIGKILL)
                                killed.append(child.pid)
                            except ProcessLookupError:
                                pass
                            if status is None:
                                try:
                                    _, status, usage = os.wait4(child.pid, 0)
                                except ChildProcessError:
                                    pass
                    finally:
                        stop.set()
                        if watch:
                            watch.join()
                        if child:
                            child.returncode = os.waitstatus_to_exitcode(status) if status is not None else -9
                        for stream in (out, err, trace):
                            stream.flush()
                            os.fsync(stream.fileno())
            group_empty = True
            if child:
                try:
                    os.killpg(child.pid, 0)
                except ProcessLookupError:
                    pass
                else:
                    group_empty = False
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    until = time.monotonic() + 2
                    while time.monotonic() < until:
                        try:
                            os.killpg(child.pid, 0)
                        except ProcessLookupError:
                            group_empty = True
                            break
                        time.sleep(0.025)
            row.update(state="completed", exit_code=child.returncode if child else None,
                       elapsed_diagnostic_seconds=time.monotonic() - started,
                       guard_reasons=reasons, exception=failure,
                       owned_child_reaped=status is not None, process_group_empty=group_empty,
                       killed_owned_process_groups=killed,
                       observed_wait4_peak_RSS_bytes=usage.ru_maxrss * 1024 if usage else None,
                       raw={name: {"path": str(folder / name), "sha256": digest(folder / name),
                                   "bytes": (folder / name).stat().st_size}
                            for name in ("stdout", "stderr", "trace.jsonl")})
            event(row)
            save()  # Persist actual raw outcome before any interpretation/assertion.
            assert failure is None and status is not None and group_empty
            assert child is not None
            if is_control:
                row["unit_result"] = json.loads((folder / "stdout").read_text())
                row["diagnostic_outcome"] = "native-controls-complete"
                save()
                assert not reasons and child.returncode == 0
                assert row["unit_result"]["passed"] is True
                assert row["unit_result"]["pure_logical_controls"] == 14
                assert row["unit_result"]["native_pixel_controls"] == 2
                bookend()
                continue
            traces = [json.loads(x) for x in (folder / "trace.jsonl").read_text().splitlines()]
            if traces:
                first = traces[0]
                assert first == {"stage": "module-imported", "pdfjs": "6.4.299",
                                 "node": "v26.8.1", "canvas": "1.0.11"}
            row["last_stage"] = traces[-1] if traces else None
            row["diagnostic_outcome"] = ("timeout" if "20-second-deadline" in reasons else
                                         "diagnostic-budget" if reasons else
                                         "nonzero-unclassified" if child.returncode else
                                         "output-returned" if target.is_file() else "missing-output")
            row["G01_observation_complete"] = False
            if target.is_file():
                row["raw_output"] = {"path": str(target), "sha256": digest(target),
                                     "bytes": target.stat().st_size}
                if target.stat().st_size <= 64 * 1024 * 1024 and not reasons and child.returncode == 0:
                    actual = images.normalize(target.read_bytes())
                    expected = images.normalize((source / case["oracle"]).read_bytes())
                    row["pixel_comparison"] = images.verify_pixels(actual, expected, case["tolerance"])
                    row["G01_observation_complete"] = True
                    normalized = f"P6\n{actual[0]} {actual[1]}\n255\n".encode() + actual[2]
                    normalized_path = folder / "normalized-actual.ppm"
                    normalized_path.write_bytes(normalized)
                    row["normalized_PPM_sha256"] = digest(normalized_path)
                    if case["id"] in CONFIG["prior_actual_PPMs"]:
                        prior = CONFIG["prior_actual_PPMs"][case["id"]]
                        prior_path = prior_images / prior["relative_path"]
                        row["prior_full_normalized_PPM_equal"] = normalized == prior_path.read_bytes()
                        row["prior_actual_PPM_sha256"] = prior["sha256"]
            save()
            bookend()
        assert len(state["rows"]) == state["all_child_processes_started"] == 14
        assert state["render_processes_started"] == 13
        assert state["native_control_processes_started"] == 1
        bookend()
        rendered = [x for x in state["rows"] if x["role"] == "pdfjs-render"]
        preserved = [x for x in rendered if x["case"] in CONFIG["prior_actual_PPMs"]]
        highwidth = next(x for x in rendered if x["case"] == "packed-mask.highwidth")
        state["cleanup_validation"] = {
            "nine_prior_full_PPMs_preserved": len(preserved) == 9 and
                all(x.get("prior_full_normalized_PPM_equal") is True for x in preserved),
            "highwidth_complete_output": highwidth["G01_observation_complete"],
            "highwidth_independent_gold_verdict": highwidth.get("pixel_comparison"),
            "native_controls_passed": state["rows"][0]["unit_result"]["passed"],
        }
        state["remaining_G01_failures"] = [x["case"] for x in rendered
                                           if not x["G01_observation_complete"]]
        state["capture_complete"] = True
        state["cleanup_validation_passed"] = (
            state["cleanup_validation"]["nine_prior_full_PPMs_preserved"] and
            state["cleanup_validation"]["highwidth_complete_output"] and
            state["cleanup_validation"]["native_controls_passed"])
        save()
        assert state["cleanup_validation_passed"], "cleanup preservation/completeness failed; raw rows retained"
    except BaseException as exc:
        state["failure"] = repr(exc)
        try:
            bookend()
            state["failure_bookend"] = "passed"
        except BaseException as bookend_failure:
            state["failure_bookend"] = repr(bookend_failure)
        save()
        raise


if __name__ == "__main__":
    main()
