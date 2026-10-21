# Current combined stack versus Hayro

Product: `402e3afe`. Hayro: standalone 0.8.0. All 78 cells and 16,380 attempts were independently audited.
100 measured pairs and five warmups per case/mode; fresh process, page zero, 72 DPI, independent RGB over white.
Primary time includes opening and rendering/preparation. Ratios are Pagura/Hayro; lower is faster or smaller.
The renderer was unchanged. This selected desktop comparison does not grant release or global superiority admission.

| Panel | Mode | Pagura correct | Hayro correct | Both correct | Median case time ratio | Faster median cases | Peak RSS no larger |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy19 | exact | 19/19 | 13/19 | 13 | 1.1543 | 0/13 | 8/13 |
| legacy19 | competitive | 18/19 | 13/19 | 13 | 1.1842 | 0/13 | 8/13 |
| interval20 | exact | 20/20 | 20/20 | 20 | 2.0216 | 0/20 | 1/20 |
| interval20 | competitive | 20/20 | 20/20 | 20 | 1.6447 | 1/20 | 1/20 |

Accuracy requires every warmup and measured output to match the frozen independent RGB policy. A mismatch may reflect a policy difference; it is not alone proof of a PDF-standard violation.
Every case follows, including incorrect or failed outputs. Quality-unqualified times are descriptive and cannot support a correctness-qualified speed claim.

| Case | Mode | Accuracy | Pagura/Hayro median ms | Median time ratio | p95 time ratio | Median upper | p95 upper | Max RSS ratio |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| interval/clip-churn | competitive | both-engines-correct | 13.5993/4.0240 | 3.3796 | 3.4059 | 3.3999 | 3.4742 | 2.6685 |
| interval/clip-churn | exact | both-engines-correct | 9.4028/4.0344 | 2.3306 | 2.4575 | 2.3514 | 2.5217 | 2.3056 |
| interval/default-knockout64 | competitive | both-engines-correct | 20.5628/3.0779 | 6.6807 | 6.8445 | 6.8086 | 7.9115 | 4.0386 |
| interval/default-knockout64 | exact | both-engines-correct | 17.7277/2.9586 | 5.9920 | 7.2979 | 6.3204 | 8.9525 | 3.5686 |
| interval/dense64 | competitive | both-engines-correct | 6.9902/3.8902 | 1.7969 | 1.7633 | 1.8132 | 1.8447 | 2.3975 |
| interval/dense64 | exact | both-engines-correct | 8.5832/3.9105 | 2.1949 | 2.0796 | 2.2128 | 2.3433 | 2.4574 |
| interval/f32-pattern-dense | competitive | both-engines-correct | 19.5428/0.3614 | 54.0696 | 54.1007 | 55.4224 | 55.0668 | 2.3398 |
| interval/f32-pattern-dense | exact | both-engines-correct | 21.7496/0.3464 | 62.7959 | 61.6909 | 63.2020 | 77.9257 | 4.8178 |
| interval/f32-shading-dense | competitive | both-engines-correct | 20.3486/0.3622 | 56.1793 | 48.4594 | 57.0968 | 53.8671 | 2.3745 |
| interval/f32-shading-dense | exact | both-engines-correct | 19.4439/0.3452 | 56.3218 | 53.7197 | 56.9202 | 56.2639 | 2.3760 |
| interval/image-only | competitive | both-engines-correct | 4.6527/0.3994 | 11.6505 | 12.3810 | 11.8734 | 14.3054 | 1.8687 |
| interval/image-only | exact | both-engines-correct | 5.0533/0.4078 | 12.3911 | 12.2211 | 12.7228 | 12.6650 | 1.6525 |
| interval/nested64 | competitive | both-engines-correct | 6.6679/3.1980 | 2.0850 | 2.0355 | 2.1093 | 2.0731 | 2.4412 |
| interval/nested64 | exact | both-engines-correct | 7.6620/3.2898 | 2.3290 | 2.2903 | 2.3517 | 2.7279 | 2.4426 |
| interval/one-source64 | competitive | both-engines-correct | 3.0816/2.8980 | 1.0633 | 1.1056 | 1.0803 | 1.1560 | 2.2895 |
| interval/one-source64 | exact | both-engines-correct | 4.1133/3.0117 | 1.3657 | 1.3822 | 1.3820 | 1.5528 | 2.3852 |
| interval/opaque-clip | competitive | both-engines-correct | 13.4780/4.0406 | 3.3356 | 3.3485 | 3.3527 | 3.4388 | 2.2737 |
| interval/opaque-clip | exact | both-engines-correct | 8.3124/3.7761 | 2.2013 | 2.2997 | 2.2152 | 2.4330 | 2.3123 |
| interval/opaque-distant | competitive | both-engines-correct | 0.2598/0.2007 | 1.2947 | 1.2577 | 1.3322 | 1.3960 | 1.4314 |
| interval/opaque-distant | exact | both-engines-correct | 0.2664/0.1992 | 1.3372 | 1.1828 | 1.3686 | 1.5741 | 1.5534 |
| interval/opaque-empty | competitive | both-engines-correct | 0.1949/0.1819 | 1.0716 | 1.0099 | 1.1015 | 1.1504 | 1.4049 |
| interval/opaque-empty | exact | both-engines-correct | 0.1932/0.1819 | 1.0622 | 1.0584 | 1.0845 | 1.1367 | 1.7353 |
| interval/opaque-fill | competitive | both-engines-correct | 0.2933/0.2165 | 1.3550 | 1.5652 | 1.3925 | 1.6807 | 1.6169 |
| interval/opaque-fill | exact | both-engines-correct | 0.3178/0.2435 | 1.3054 | 1.2138 | 1.3437 | 2.1027 | 1.8858 |
| interval/opaque-tiny | competitive | both-engines-correct | 0.1060/0.0935 | 1.1339 | 1.2411 | 1.1717 | 1.7669 | 1.0000 |
| interval/opaque-tiny | exact | both-engines-correct | 0.1136/0.0982 | 1.1572 | 1.4255 | 1.1780 | 2.0672 | 0.8894 |
| interval/opaque64 | competitive | both-engines-correct | 2.7399/2.9251 | 0.9367 | 0.9005 | 0.9403 | 0.9379 | 2.2069 |
| interval/opaque64 | exact | both-engines-correct | 3.7248/3.1279 | 1.1908 | 1.1635 | 1.2003 | 1.2016 | 2.2618 |
| interval/outline2 | competitive | both-engines-correct | 1.5981/1.5649 | 1.0212 | 1.0248 | 1.0308 | 1.0578 | 1.6918 |
| interval/outline2 | exact | both-engines-correct | 3.0598/1.6555 | 1.8482 | 1.7724 | 1.8620 | 1.8462 | 1.9954 |
| interval/outline2-knockout | competitive | both-engines-correct | 6.5741/1.5540 | 4.2306 | 4.6941 | 4.2613 | 5.3161 | 2.9931 |
| interval/outline2-knockout | exact | both-engines-correct | 6.3131/1.6967 | 3.7208 | 4.2861 | 3.7712 | 4.5184 | 3.5068 |
| interval/outline2-late64 | competitive | both-engines-correct | 1.6821/1.6258 | 1.0346 | 1.0834 | 1.0517 | 1.1200 | 1.8468 |
| interval/outline2-late64 | exact | both-engines-correct | 2.9110/1.6100 | 1.8081 | 1.8641 | 1.8238 | 1.9213 | 1.6786 |
| interval/overcapacity128 | competitive | both-engines-correct | 7.7330/4.0181 | 1.9245 | 2.2102 | 1.9457 | 2.7024 | 2.5855 |
| interval/overcapacity128 | exact | both-engines-correct | 9.2116/4.0025 | 2.3014 | 2.3657 | 2.3241 | 2.4301 | 2.2996 |
| interval/sparse64 | competitive | both-engines-correct | 3.4708/2.9280 | 1.1854 | 1.2262 | 1.1986 | 1.3595 | 2.3565 |
| interval/sparse64 | exact | both-engines-correct | 4.2802/3.0179 | 1.4183 | 1.4710 | 1.4461 | 1.6173 | 2.4238 |
| interval/tiny | competitive | both-engines-correct | 0.2543/0.1704 | 1.4925 | 1.5069 | 1.5181 | 1.5819 | 1.0333 |
| interval/tiny | exact | both-engines-correct | 0.2686/0.1733 | 1.5500 | 1.6221 | 1.5707 | 2.0023 | 1.0280 |
| classic/blank | competitive | both-engines-correct | 0.1000/0.0885 | 1.1301 | 1.1509 | 1.1564 | 1.3467 | 0.9027 |
| classic/blank | exact | both-engines-correct | 0.0844/0.0814 | 1.0361 | 1.0219 | 1.0492 | 1.2064 | 0.8929 |
| classic/clipped-to-page | competitive | both-engines-correct | 0.1083/0.0976 | 1.1092 | 1.0557 | 1.1405 | 1.1776 | 0.9827 |
| classic/clipped-to-page | exact | both-engines-correct | 0.1083/0.0938 | 1.1543 | 1.3232 | 1.1852 | 2.0249 | 0.9418 |
| classic/content-array-split | competitive | both-engines-correct | 0.1177/0.1005 | 1.1717 | 1.2163 | 1.2284 | 1.4500 | 1.0000 |
| classic/content-array-split | exact | both-engines-correct | 0.1095/0.0991 | 1.1051 | 1.0869 | 1.1298 | 1.1912 | 0.9655 |
| classic/graphics-state | competitive | both-engines-correct | 0.1118/0.0919 | 1.2168 | 1.2599 | 1.2495 | 1.3109 | 0.9548 |
| classic/graphics-state | exact | both-engines-correct | 0.1068/0.0950 | 1.1246 | 1.1606 | 1.1437 | 1.2301 | 0.9649 |
| classic/inherited-box | competitive | both-engines-correct | 0.1105/0.0963 | 1.1468 | 0.5552 | 1.1753 | 1.0940 | 0.8836 |
| classic/inherited-box | exact | both-engines-correct | 0.1129/0.0989 | 1.1409 | 1.1600 | 1.1765 | 1.2216 | 0.9441 |
| classic/many-operands | competitive | both-engines-correct | 19.5470/6.8070 | 2.8716 | 2.8156 | 2.9104 | 2.8710 | 2.9013 |
| classic/many-operands | exact | both-engines-correct | 16.0462/6.8037 | 2.3584 | 2.4180 | 2.3897 | 2.4629 | 2.8671 |
| classic/many-rectangles | competitive | both-engines-correct | 1.5762/0.9428 | 1.6719 | 1.7193 | 1.6987 | 1.7660 | 1.5163 |
| classic/many-rectangles | exact | both-engines-correct | 1.4431/0.9855 | 1.4644 | 1.5304 | 1.4930 | 2.1059 | 1.6596 |
| classic/negative-rectangle | competitive | both-engines-correct | 0.1129/0.0947 | 1.1918 | 1.0535 | 1.2197 | 1.1586 | 0.9777 |
| classic/negative-rectangle | exact | both-engines-correct | 0.0978/0.0866 | 1.1289 | 1.1727 | 1.1401 | 1.2832 | 0.9532 |
| classic/rectangle | competitive | both-engines-correct | 0.1080/0.0923 | 1.1697 | 1.2147 | 1.1940 | 1.3496 | 0.9653 |
| classic/rectangle | exact | both-engines-correct | 0.1512/0.1319 | 1.1461 | 1.1546 | 1.1745 | 1.2990 | 0.9563 |
| paint/image-direct-bilinear | competitive | quality-unqualified | 0.1054/0.1058 | 0.9959 | 1.0252 | 1.0100 | 1.1124 | 0.9130 |
| paint/image-direct-bilinear | exact | quality-unqualified | 0.1119/0.1007 | 1.1109 | 1.2101 | 1.1203 | 1.4186 | 0.9830 |
| paint/image-direct-binary-halves | competitive | quality-unqualified | 0.1190/0.1277 | 0.9317 | 0.9531 | 0.9477 | 1.0262 | 0.9263 |
| paint/image-direct-binary-halves | exact | quality-unqualified | 0.1158/0.1172 | 0.9886 | 1.0047 | 1.0052 | 1.1016 | 0.9010 |
| paint/image-direct-gray-quarter-turn | competitive | quality-unqualified | 0.1198/0.1250 | 0.9590 | 0.9743 | 0.9863 | 1.1087 | 0.9158 |
| paint/image-direct-gray-quarter-turn | exact | quality-unqualified | 0.1154/0.1153 | 1.0016 | 1.1100 | 1.0310 | 1.2736 | 0.9259 |
| paint/image-direct-rgb-fractional | competitive | quality-unqualified | 0.1196/0.1259 | 0.9500 | 0.9331 | 0.9629 | 1.0316 | 0.8629 |
| paint/image-direct-rgb-fractional | exact | quality-unqualified | 0.1194/0.1164 | 1.0263 | 1.0913 | 1.0551 | 1.1374 | 0.9418 |
| paint/image-soft-mask | competitive | both-engines-correct | 0.1230/0.1038 | 1.1842 | 1.1435 | 1.2091 | 1.2495 | 0.9835 |
| paint/image-soft-mask | exact | both-engines-correct | 0.1259/0.1072 | 1.1741 | 1.2147 | 1.2077 | 1.2710 | 0.9786 |
| paint/text-heavy-page | competitive | both-engines-correct | 0.4291/0.4109 | 1.0443 | 1.0366 | 1.0540 | 1.0838 | 1.1347 |
| paint/text-heavy-page | exact | both-engines-correct | 0.6007/0.4030 | 1.4905 | 1.5359 | 1.5196 | 1.5879 | 1.1847 |
| paint/text-knockout-false | competitive | quality-unqualified | 0.2111/0.1358 | 1.5547 | 1.3786 | 1.5844 | 1.5397 | 0.9951 |
| paint/text-knockout-false | exact | quality-unqualified | 0.1890/0.1233 | 1.5326 | 1.3925 | 1.5698 | 1.5144 | 1.0099 |
| paint/text-knockout-true | competitive | quality-unqualified | 0.2238/0.1337 | 1.6743 | 1.6449 | 1.7114 | 2.1107 | 1.1782 |
| paint/text-knockout-true | exact | quality-unqualified | 0.2765/0.1792 | 1.5427 | 1.4748 | 1.5781 | 1.8727 | 1.0280 |
| paint/text-mode-4 | competitive | both-engines-correct | 0.2028/0.1278 | 1.5865 | 1.5509 | 1.6106 | 1.7921 | 1.0647 |
| paint/text-mode-4 | exact | both-engines-correct | 0.1984/0.1253 | 1.5831 | 1.5535 | 1.6144 | 1.6254 | 1.0299 |
| paint/type3-d0-colour | competitive | both-engines-correct | 0.1684/0.1218 | 1.3822 | 1.3263 | 1.4101 | 1.5612 | 1.0415 |
| paint/type3-d0-colour | exact | both-engines-correct | 0.1519/0.1101 | 1.3792 | 1.4167 | 1.3987 | 1.6761 | 1.1087 |

[All distributions, mismatch details and evidence bindings](current-stack-hayro.json).

- Fixed synthetic workload panel; no universal PDF correctness or global engine-superiority claim.
- Exact and Competitive are separate cells; Hayro is sampled afresh in both cells with identical settings.
- Primary metric is open plus render; render-only and full process timings are separately retained.
- All 100 measured pairs contribute or that metric is unavailable; wrong-output timings remain explicitly unqualified.
- All five warmups contribute to accuracy qualification; no sample is removed for an unfavorable value.
- RGB-over-white comparison uses frozen independent references at zero tolerance; policy disagreement alone is not a PDF violation.
- Pagura P6 carries no alpha; Hayro alpha is observed without an independent alpha-superiority claim.
- RSS is raw per-child Darwin /usr/bin/time maximum resident bytes; distributions are descriptive, with no confidence interval.
- Point ratios are descriptive, without bootstrap bounds or release admission.
- Retained diagnostics are the combined text returned by run_process, not separately captured original stdout/stderr streams.
- The cooperative observer cannot exclude ordinary desktop interference or activity between observations.
- Primary bootstrap uses the prospectively frozen existing statistics implementation. Bounds are not independently recalculated by the raw-evidence auditor.
- Bounds are marginal 95% one-sided endpoints, without a family-wide or temporal-dependence guarantee.
- Historical ratios come from an earlier run and adapter build; changes in ratios do not isolate a causal renderer speedup.
- Case-ratio medians are unweighted descriptions of each matching subset; cohort and mode subsets may differ.
