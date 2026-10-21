# F32 painted-interval amended confirmation

**Coordinator decision:** retain-experimentally.

All mandatory accuracy, resource, layout and portability gates passed. The independent audit verified all 22,960 scheduled outputs and retained all 3,310 completed original pairs plus 8,170 new pairs. Both designated sparse64/full latency gates pass: Exact median 7.668646 to 3.8338955 ms (50.01% less), Competitive 6.825750 to 3.0674165 ms (55.06% less). Retain the incremental output-conversion saving as a useful partial win under the explicit user-authorized continuation amendment.

Experimental branch only. No main/default/release or global superiority admission: only 17/54 protected latency and 30/56 sampled RSS gates pass. Exact target maximum RSS rises 6.60%; largest control maximum RSS rises 25.32%. Competitive opaque-tiny/cold median ratio 1.07483 with lower bound 1.05706 and opaque-clip/full p95 ratio 1.22510 with lower bound 1.04296 exceed the material-regression threshold. These misses remain unresolved; do not excuse them as noise or infer zero impact from the original presence-only guard. Cohort and checkpoint methods differ explicitly. Pause after publication and commit per the user; no further performance experiment.

Retention, if selected, is experimental only; no main/default/release promotion.

The original segment stopped when its observer detected another process. Process presence alone establishes neither CPU use nor a timing effect. The user explicitly authorized retaining every completed pair and finishing the original fixed schedule; no timing-based sample selection was used. The failed original receipt/guard and any incomplete terminal attempt remain linked in the JSON.

Two cohorts: 3310 complete original pairs and 8170 continuation pairs, including the original five warmups per cell. This is neither uninterrupted nor wholly fresh confirmation. The continuation receipt/guard and combined independent audit passed.

Unchanged numeric gates: target latency 2/2; protected latency 17/54; sampled peak RSS 30/56. All 22,960 scheduled outputs independently checked. Numeric all-cell pass: false. These pooled gates do not remove the cohort limitation. No release admission.

Retain every miss: 37 latency cells and 26 RSS cells. 2 cells have a median or p95 lower bound above 1.02; these are listed in the JSON. Upper-bound misses alone do not establish a regression. RSS misses remain unresolved observations and are not excused by layout evidence.

[All statistics, decisions and exact evidence pins](f32-painted-interval-continuation-experiment.json). Ratios are candidate/baseline; lower is faster or smaller. Bounds are marginal 95% one-sided endpoints.

Supported >2% signal: opaque-tiny / competitive / cold (protected): median lower 1.057063.

Supported >2% signal: opaque-clip / competitive / full (protected): p95 lower 1.042955.

| Case | Mode | Operation | Role | Baseline/candidate median ms | Median ratio [lower, upper] | p95 ratio [lower, upper] | Latency gate |
| --- | --- | --- | --- | ---: | ---: | ---: | --- |
| image-only | competitive | full | protected | 4.253729/4.274979 | 1.004996 [0.997406, 1.015427] | 1.016576 [1.005157, 1.028377] | miss |
| tiny | competitive | full | protected | 0.047166/0.044562 | 0.944801 [0.932070, 0.961589] | 0.975230 [0.917081, 1.043415] | miss |
| opaque-distant | exact | full | protected | 0.166229/0.167937 | 1.010278 [1.000626, 1.023189] | 1.089694 [0.989267, 1.187076] | miss |
| outline2-late64 | competitive | full | protected | 1.202667/1.146084 | 0.952952 [0.943383, 0.977780] | 0.982494 [0.942002, 1.033634] | miss |
| outline2 | exact | full | protected | 2.433146/2.380270 | 0.978269 [0.971934, 0.985919] | 0.978067 [0.899848, 1.167752] | miss |
| one-source64 | exact | full | protected | 7.009230/3.531042 | 0.503770 [0.496263, 0.511923] | 0.511802 [0.439076, 0.578483] | pass |
| sparse64 | exact | cold | protected | 8.294021/4.461437 | 0.537910 [0.528390, 0.548116] | 0.563122 [0.432666, 0.649174] | pass |
| clip-churn | competitive | full | protected | 8.180958/8.177416 | 0.999567 [0.997459, 1.002363] | 1.001616 [0.963238, 1.026220] | miss |
| sparse64 | competitive | tiled | protected | 11.307709/8.052812 | 0.712152 [0.709487, 0.715263] | 0.763904 [0.700703, 0.897270] | pass |
| sparse64 | exact | tiled | protected | 11.455438/8.166479 | 0.712891 [0.710946, 0.714168] | 0.632959 [0.550045, 0.716608] | pass |
| tiny | exact | cold | protected | 0.262646/0.256854 | 0.977948 [0.971582, 0.986947] | 0.940759 [0.866519, 1.019704] | pass |
| f32-shading-dense | exact | full | protected | 20.656542/20.591041 | 0.996829 [0.988546, 1.009389] | 1.042275 [1.011372, 1.081955] | miss |
| tiny | competitive | cold | protected | 0.252542/0.248125 | 0.982510 [0.970606, 0.993838] | 0.925568 [0.829196, 1.062274] | miss |
| opaque-fill | exact | full | protected | 0.223833/0.225645 | 1.008098 [0.987989, 1.030164] | 1.097515 [0.797823, 2.323360] | miss |
| outline2-knockout | exact | full | protected | 5.497333/5.582375 | 1.015470 [0.995375, 1.031404] | 0.918047 [0.794239, 1.240573] | miss |
| f32-pattern-dense | competitive | full | protected | 21.153250/21.278709 | 1.005931 [0.997036, 1.013773] | 1.040872 [0.925664, 1.388017] | miss |
| sparse64 | exact | full | target | 7.668646/3.833896 | 0.499944 [0.489197, 0.507366] | 0.525478 [0.438859, 0.586478] | pass |
| outline2-knockout | competitive | full | protected | 7.540958/7.442520 | 0.986946 [0.969601, 1.025744] | 0.813742 [0.582696, 1.052853] | miss |
| dense64 | competitive | full | protected | 10.796229/5.977625 | 0.553677 [0.537268, 0.569339] | 0.753074 [0.509119, 1.013456] | pass |
| opaque-clip | exact | full | protected | 5.617354/5.647500 | 1.005367 [0.961823, 1.041914] | 0.959992 [0.807941, 1.283966] | miss |
| sparse64 | exact | tile | protected | 0.269271/0.214875 | 0.797989 [0.783630, 0.824330] | 1.035420 [0.579971, 1.217673] | miss |
| opaque-tiny | competitive | cold | protected | 0.091875/0.098750 | 1.074830 [1.057063, 1.094373] | 0.993193 [0.930962, 1.109332] | miss |
| image-only | exact | full | protected | 4.840000/4.924251 | 1.017407 [0.988648, 1.049641] | 0.990305 [0.861041, 1.187339] | miss |
| opaque-distant | competitive | full | protected | 0.183958/0.182188 | 0.990373 [0.978275, 1.004555] | 1.037284 [0.958978, 1.109548] | miss |
| sparse64 | competitive | cold | protected | 7.544625/3.768896 | 0.499547 [0.495104, 0.505953] | 0.645829 [0.501027, 0.791862] | pass |
| sparse64 | competitive | tile | protected | 0.284500/0.234271 | 0.823448 [0.810917, 0.832891] | 0.898246 [0.794654, 1.055508] | miss |
| one-source64 | competitive | full | protected | 6.954229/2.978833 | 0.428348 [0.423310, 0.435561] | 0.420540 [0.394128, 0.496898] | pass |
| opaque64 | competitive | full | protected | 2.379729/2.407646 | 1.011731 [0.989762, 1.033746] | 1.053819 [0.857819, 1.161901] | miss |
| nested64 | competitive | full | protected | 10.712229/6.619230 | 0.617913 [0.609647, 0.626029] | 0.587159 [0.480619, 0.638181] | pass |
| overcapacity128 | exact | full | protected | 11.305438/9.321687 | 0.824531 [0.817071, 0.833049] | 0.811214 [0.733515, 1.010169] | pass |
| sparse64 | competitive | full | target | 6.825750/3.067417 | 0.449389 [0.444562, 0.458728] | 0.458840 [0.428628, 0.506190] | pass |
| f32-pattern-dense | exact | full | protected | 24.402875/24.386625 | 0.999334 [0.992954, 1.008373] | 0.936796 [0.827657, 1.006355] | pass |
| opaque64 | competitive | cold | protected | 3.025979/3.018729 | 0.997604 [0.990456, 1.008631] | 1.037306 [0.972590, 1.138540] | miss |
| outline2-late64 | exact | full | protected | 2.734709/2.683813 | 0.981389 [0.969545, 0.989706] | 1.057715 [0.995327, 1.154737] | miss |
| default-knockout64 | exact | full | protected | 19.789188/19.943750 | 1.007810 [0.991094, 1.032591] | 1.050832 [0.914251, 1.253916] | miss |
| dense64 | exact | full | protected | 10.478751/6.565645 | 0.626568 [0.621731, 0.631460] | 0.649605 [0.581502, 0.749965] | pass |
| opaque64 | competitive | tile | protected | 0.188562/0.189916 | 1.007181 [0.996558, 1.020388] | 0.997610 [0.969460, 1.033045] | miss |
| outline2 | competitive | full | protected | 1.177812/1.135709 | 0.964252 [0.957744, 0.968844] | 0.947917 [0.924875, 0.979014] | pass |
| opaque64 | competitive | tiled | protected | 7.450292/7.461146 | 1.001457 [0.996321, 1.008570] | 0.964793 [0.852824, 1.065557] | miss |
| opaque64 | exact | cold | protected | 3.741687/3.751605 | 1.002650 [0.989284, 1.013689] | 0.747302 [0.621649, 1.014887] | pass |
| opaque-empty | competitive | full | protected | 0.137792/0.136083 | 0.987601 [0.966194, 1.011510] | 0.911265 [0.717714, 1.300400] | miss |
| opaque64 | exact | full | protected | 3.090292/3.108729 | 1.005966 [0.992045, 1.017882] | 1.005525 [0.894513, 1.272377] | miss |
| opaque-tiny | competitive | full | protected | 0.025875/0.025583 | 0.988734 [0.975962, 1.003257] | 1.019282 [0.989259, 1.085934] | miss |
| opaque64 | exact | tile | protected | 0.153584/0.154854 | 1.008272 [0.989926, 1.026578] | 0.987822 [0.935237, 1.024273] | miss |
| f32-shading-dense | competitive | full | protected | 19.461771/19.398688 | 0.996759 [0.995331, 0.998075] | 1.001769 [0.987474, 1.013860] | pass |
| nested64 | exact | full | protected | 10.056563/6.570042 | 0.653309 [0.641284, 0.663173] | 0.577217 [0.461645, 0.710631] | pass |
| default-knockout64 | competitive | full | protected | 19.216250/19.069395 | 0.992358 [0.980060, 1.003259] | 0.986118 [0.770960, 1.093443] | miss |
| opaque-tiny | exact | cold | protected | 0.080229/0.080459 | 1.002867 [0.987692, 1.009660] | 0.986523 [0.947377, 1.083613] | miss |
| opaque-tiny | exact | full | protected | 0.023645/0.024021 | 1.015859 [0.994780, 1.029989] | 0.983382 [0.948757, 1.037176] | miss |
| clip-churn | exact | full | protected | 4.356250/4.337458 | 0.995686 [0.992711, 1.000139] | 0.971408 [0.919964, 0.994511] | pass |
| opaque-empty | exact | full | protected | 0.118500/0.119063 | 1.004747 [0.984309, 1.018652] | 0.995050 [0.880570, 1.117507] | miss |
| opaque-fill | competitive | full | protected | 0.209938/0.212438 | 1.011906 [0.992440, 1.026778] | 1.012454 [0.899753, 1.119339] | miss |
| opaque64 | exact | tiled | protected | 7.495271/7.522458 | 1.003627 [1.000495, 1.010021] | 0.996112 [0.870630, 1.117930] | miss |
| tiny | exact | full | protected | 0.051021/0.047708 | 0.935075 [0.921382, 0.952029] | 0.944593 [0.761520, 1.271364] | miss |
| opaque-clip | competitive | full | protected | 9.217792/9.191937 | 0.997195 [0.982519, 1.005080] | 1.225105 [1.042955, 1.495089] | miss |
| overcapacity128 | competitive | full | protected | 9.149646/7.151396 | 0.781604 [0.772372, 0.792453] | 1.002083 [0.804473, 1.334462] | miss |

RSS values below are min / median / p95 / max MiB from all 200 measured samples in each arm. The unchanged RSS gate uses the ratio of maxima.

| Case | Mode | Operation | Baseline RSS MiB | Candidate RSS MiB | Max ratio | RSS gate |
| --- | --- | --- | ---: | ---: | ---: | --- |
| image-only | competitive | full | 3.969 / 3.969 / 4.016 / 4.688 | 3.938 / 3.938 / 4.297 / 5.016 | 1.070000 | miss |
| tiny | competitive | full | 3.141 / 3.141 / 3.188 / 3.547 | 3.109 / 3.109 / 3.156 / 3.484 | 0.982379 | pass |
| opaque-distant | exact | full | 3.906 / 3.906 / 3.906 / 4.625 | 3.922 / 3.922 / 3.924 / 4.578 | 0.989865 | pass |
| outline2-late64 | competitive | full | 6.375 / 6.391 / 6.391 / 8.578 | 6.359 / 6.375 / 6.438 / 10.750 | 1.253188 | miss |
| outline2 | exact | full | 6.297 / 6.297 / 8.502 / 12.812 | 6.250 / 6.250 / 8.454 / 10.672 | 0.832927 | pass |
| one-source64 | exact | full | 20.312 / 20.406 / 24.641 / 29.969 | 20.266 / 20.359 / 22.546 / 29.953 | 0.999479 | pass |
| sparse64 | exact | cold | 21.219 / 21.344 / 30.815 / 33.156 | 21.172 / 21.266 / 27.555 / 32.141 | 0.969369 | pass |
| clip-churn | competitive | full | 11.172 / 11.172 / 12.313 / 13.547 | 11.156 / 11.156 / 12.359 / 13.656 | 1.008074 | miss |
| sparse64 | competitive | tiled | 22.250 / 22.312 / 26.798 / 45.578 | 22.219 / 22.281 / 27.520 / 45.969 | 1.008570 | miss |
| sparse64 | exact | tiled | 21.469 / 21.531 / 25.782 / 35.547 | 21.422 / 21.484 / 24.161 / 29.562 | 0.831648 | pass |
| tiny | exact | cold | 3.219 / 3.219 / 3.297 / 3.469 | 3.172 / 3.172 / 3.236 / 3.438 | 0.990991 | pass |
| f32-shading-dense | exact | full | 4.484 / 5.648 / 7.923 / 8.078 | 4.469 / 5.656 / 7.923 / 8.078 | 1.000000 | pass |
| tiny | competitive | cold | 3.141 / 3.141 / 3.250 / 3.641 | 3.109 / 3.109 / 3.156 / 3.469 | 0.952790 | pass |
| opaque-fill | exact | full | 3.906 / 3.906 / 4.547 / 5.234 | 3.922 / 3.922 / 4.518 / 5.672 | 1.083582 | miss |
| outline2-knockout | exact | full | 9.141 / 9.188 / 21.313 / 29.750 | 9.094 / 9.094 / 21.594 / 32.297 | 1.085609 | miss |
| f32-pattern-dense | competitive | full | 4.547 / 6.672 / 7.828 / 8.062 | 4.531 / 6.688 / 7.859 / 8.156 | 1.011628 | miss |
| sparse64 | exact | full | 21.219 / 22.500 / 28.720 / 31.031 | 21.172 / 21.312 / 26.582 / 33.078 | 1.065962 | miss |
| outline2-knockout | competitive | full | 9.156 / 16.570 / 26.701 / 35.797 | 9.109 / 16.664 / 24.423 / 32.016 | 0.894369 | pass |
| dense64 | competitive | full | 22.062 / 26.547 / 30.033 / 33.094 | 21.984 / 25.297 / 30.849 / 34.234 | 1.034466 | miss |
| opaque-clip | exact | full | 10.672 / 11.484 / 12.641 / 12.938 | 10.656 / 11.469 / 12.609 / 12.969 | 1.002415 | miss |
| sparse64 | exact | tile | 5.453 / 5.453 / 5.906 / 6.203 | 5.406 / 5.406 / 5.752 / 5.969 | 0.962217 | pass |
| opaque-tiny | competitive | cold | 2.406 / 2.406 / 2.562 / 2.656 | 2.422 / 2.422 / 2.548 / 2.734 | 1.029412 | miss |
| image-only | exact | full | 3.969 / 4.328 / 5.613 / 6.125 | 3.938 / 4.297 / 5.707 / 7.109 | 1.160714 | miss |
| opaque-distant | competitive | full | 3.906 / 3.906 / 3.953 / 5.359 | 3.922 / 3.922 / 3.969 / 4.172 | 0.778426 | pass |
| sparse64 | competitive | cold | 21.672 / 22.039 / 25.188 / 33.812 | 21.641 / 21.805 / 25.141 / 33.906 | 1.002773 | miss |
| sparse64 | competitive | tile | 5.906 / 6.000 / 6.094 / 6.453 | 5.875 / 5.969 / 6.173 / 6.547 | 1.014528 | miss |
| one-source64 | competitive | full | 20.266 / 22.438 / 25.720 / 31.031 | 20.234 / 20.344 / 24.596 / 29.891 | 0.963243 | pass |
| opaque64 | competitive | full | 21.203 / 21.445 / 23.677 / 25.594 | 21.156 / 21.484 / 23.766 / 26.234 | 1.025031 | miss |
| nested64 | competitive | full | 21.750 / 25.141 / 27.567 / 32.875 | 21.703 / 24.023 / 27.330 / 29.688 | 0.903042 | pass |
| overcapacity128 | exact | full | 22.328 / 25.539 / 27.766 / 30.906 | 22.281 / 24.531 / 27.911 / 33.125 | 1.071790 | miss |
| sparse64 | competitive | full | 21.688 / 22.812 / 27.281 / 33.828 | 21.656 / 21.852 / 25.281 / 29.531 | 0.872979 | pass |
| f32-pattern-dense | exact | full | 7.031 / 14.156 / 17.797 / 17.984 | 7.031 / 14.188 / 17.812 / 17.938 | 0.997394 | pass |
| opaque64 | competitive | cold | 21.172 / 21.391 / 23.098 / 24.406 | 21.109 / 21.359 / 23.081 / 25.656 | 1.051216 | miss |
| outline2-late64 | exact | full | 6.391 / 6.555 / 8.579 / 11.766 | 6.359 / 6.422 / 9.612 / 11.156 | 0.948207 | pass |
| default-knockout64 | exact | full | 23.859 / 35.016 / 45.990 / 57.203 | 23.812 / 35.016 / 46.021 / 57.328 | 1.002185 | miss |
| dense64 | exact | full | 21.516 / 22.812 / 27.045 / 32.484 | 21.469 / 22.586 / 26.956 / 33.500 | 1.031265 | miss |
| opaque64 | competitive | tile | 5.391 / 5.484 / 5.562 / 5.812 | 5.344 / 5.453 / 5.531 / 5.719 | 0.983871 | pass |
| outline2 | competitive | full | 6.250 / 6.266 / 6.424 / 8.438 | 6.219 / 6.234 / 6.312 / 7.453 | 0.883333 | pass |
| opaque64 | competitive | tiled | 21.750 / 21.812 / 26.922 / 41.156 | 21.719 / 21.781 / 26.048 / 33.625 | 0.817008 | pass |
| opaque64 | exact | cold | 20.719 / 20.828 / 23.609 / 26.734 | 20.672 / 20.781 / 23.548 / 26.031 | 0.973700 | pass |
| opaque-empty | competitive | full | 3.656 / 3.656 / 3.908 / 5.406 | 3.641 / 3.641 / 3.781 / 5.312 | 0.982659 | pass |
| opaque64 | exact | full | 20.719 / 20.828 / 23.673 / 27.172 | 20.672 / 20.797 / 24.192 / 26.047 | 0.958597 | pass |
| opaque-tiny | competitive | full | 2.406 / 2.406 / 2.438 / 2.500 | 2.422 / 2.422 / 2.453 / 2.609 | 1.043750 | miss |
| opaque64 | exact | tile | 4.953 / 4.953 / 4.953 / 5.234 | 4.906 / 4.906 / 4.906 / 5.094 | 0.973134 | pass |
| f32-shading-dense | competitive | full | 4.500 / 4.500 / 6.844 / 7.953 | 4.484 / 4.484 / 6.798 / 8.016 | 1.007859 | miss |
| nested64 | exact | full | 21.266 / 21.375 / 25.738 / 33.203 | 21.219 / 21.328 / 25.531 / 33.094 | 0.996706 | pass |
| default-knockout64 | competitive | full | 23.875 / 23.938 / 38.688 / 64.750 | 23.828 / 23.891 / 38.614 / 64.516 | 0.996380 | pass |
| opaque-tiny | exact | cold | 2.406 / 2.406 / 2.406 / 2.531 | 2.422 / 2.422 / 2.422 / 2.547 | 1.006173 | miss |
| opaque-tiny | exact | full | 2.406 / 2.406 / 2.438 / 2.797 | 2.422 / 2.422 / 2.453 / 2.703 | 0.966480 | pass |
| clip-churn | exact | full | 11.172 / 11.172 / 12.267 / 13.406 | 11.156 / 11.156 / 11.266 / 13.297 | 0.991841 | pass |
| opaque-empty | exact | full | 3.641 / 3.641 / 3.688 / 4.391 | 3.625 / 3.625 / 3.672 / 3.688 | 0.839858 | pass |
| opaque-fill | competitive | full | 3.906 / 3.906 / 3.984 / 5.812 | 3.922 / 3.922 / 3.985 / 4.688 | 0.806452 | pass |
| opaque64 | exact | tiled | 20.969 / 21.031 / 23.327 / 25.516 | 20.922 / 20.984 / 23.563 / 29.641 | 1.161666 | miss |
| tiny | exact | full | 3.219 / 3.219 / 3.266 / 3.453 | 3.172 / 3.172 / 3.234 / 3.469 | 1.004525 | miss |
| opaque-clip | competitive | full | 10.672 / 11.297 / 12.142 / 12.734 | 10.656 / 11.250 / 12.516 / 12.828 | 1.007362 | miss |
| overcapacity128 | competitive | full | 22.938 / 24.266 / 28.627 / 33.922 | 22.969 / 23.320 / 28.738 / 35.062 | 1.033625 | miss |

- Synthetic incremental comparison against f9738174; no overall-stack, latest-main, competitor or global superiority claim.
- All 22960 outputs were independently checked against fixed fixture references; this is not correctness for every PDF or continuous geometry.
- Marginal bootstrap bounds are retained from the pinned harness, not independently recomputed by the audit or publication helper.
- All 200 measured pairs per cell contribute to RSS distributions; maxima have no confidence interval, and misses have no presumed cause.
- This is a post-stop user-authorized two-cohort method amendment, not uninterrupted or wholly fresh confirmation.
- The original guard stopped on another process being present; neither zero CPU consumption nor actual timing contamination is established.
- Every structurally complete original pair is retained. The terminal incomplete attempt remains recorded and its pair is rerun in full.
- The cohort/time boundary is explicit; unchanged pooled bootstrap thresholds do not establish equivalence to uninterrupted sampling.
- The continuation guard and independent audit are required to pass; observation still cannot exclude brief jobs between scans or ordinary desktop interference.
- WASI is a bounded portability/diagnostic comparison with no calibrated speed or RSS admission.
- No pilot or historical samples are pooled; no further measurement is authorized by this publication.
