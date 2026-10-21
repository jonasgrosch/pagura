# N16 painted-interval confirmation

Retained as a partial experimental win on 2026-10-21. Full admission fails.

Both targets passed; protected latency passed 8/50 cells and sampled peak RSS passed 28/52. All 21,320 independently checked outputs passed. No observed competing compute.

| Target | Baseline median ms | Candidate median ms | Less render time | Median ratio upper | p95 ratio upper | Peak RSS ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| competitive full | 3.249104 | 2.045479 | 37.04% | 0.636536 | 0.669696 | 1.096016 |
| exact full | 3.940520 | 2.782709 | 29.38% | 0.710848 | 0.743021 | 0.927885 |

The two supported protected signals are the tiny opaque cold cases: Competitive +3.50% median (81.04 to 83.88 microseconds), Exact +4.46% (85.42 to 89.23 microseconds). Their median lower bounds exceed the unchanged 1.02 limit. The other protected upper-bound misses remain failures to establish the bound, without a supported lower-bound regression.

Peak RSS misses are not excused by unchanged storage. The largest sampled ratio is 1.3229 for the Competitive outline-knockout control (24.92 to 32.97 MiB). The Competitive target also misses RSS (23.92 to 26.22 MiB). Causation is unresolved.

Experimental branch only. No main, deployed/default, release, general memory-neutrality or competitor promotion. The two supported tiny cold regressions and every protected/RSS miss remain unresolved and must be repaired or evaluated under a separately justified workload restriction before broader promotion.

[Complete machine-readable result, all 52 cells and RSS distributions](n16-painted-interval-experiment.json). [Independent validation](n16-painted-interval-quality.md). [Separately declared recovery](../../docs/plans/n16-painted-interval-recovery.md).

## All fixed cells

| Case | Mode | Operation | Median ratio | Median lower/upper | p95 lower/upper | RSS ratio | Latency/RSS gates |
| --- | --- | --- | ---: | --- | --- | ---: | --- |
| image-only | exact | full | 1.026940 | 1.015237/1.037109 | 0.967930/1.089673 | 1.059666 | miss/miss |
| dense64 | competitive | full | 0.997805 | 0.986645/1.011990 | 0.814821/1.376744 | 1.033019 | miss/miss |
| sparse64 | exact | tile | 1.006358 | 0.992274/1.022794 | 0.836721/1.007807 | 0.994609 | miss/pass |
| opaque-empty | competitive | full | 0.753881 | 0.741759/0.768306 | 0.734177/0.848408 | 0.896440 | pass/pass |
| opaque-fill | exact | full | 1.006260 | 0.991775/1.021892 | 0.858501/0.992108 | 1.163823 | miss/miss |
| sparse64 | competitive | cold | 0.999603 | 0.985084/1.015269 | 0.908713/1.202527 | 1.065912 | miss/miss |
| opaque64 | exact | tiled | 0.936892 | 0.926291/0.952740 | 0.828875/1.069260 | 0.907572 | miss/pass |
| clip-churn | competitive | full | 0.999885 | 0.987035/1.017575 | 0.698839/1.181827 | 1.008342 | miss/miss |
| dense64 | exact | full | 0.998419 | 0.993677/1.006909 | 0.776989/1.526502 | 1.012171 | miss/miss |
| sparse64 | exact | full | 1.003243 | 0.999304/1.005441 | 0.952632/1.112772 | 0.995480 | miss/pass |
| one-source64 | competitive | full | 1.001550 | 0.990618/1.008554 | 1.001063/1.100511 | 0.759717 | miss/pass |
| tiny | exact | full | 1.007636 | 0.995785/1.019952 | 0.872468/1.114678 | 1.000000 | miss/pass |
| opaque-distant | competitive | full | 0.987131 | 0.973277/1.002134 | 0.944853/1.044836 | 1.003448 | miss/miss |
| sparse64 | competitive | tiled | 0.996393 | 0.991174/1.002315 | 0.874127/1.194324 | 0.929395 | miss/pass |
| opaque64 | exact | cold | 0.734141 | 0.726111/0.740790 | 0.470966/0.781265 | 0.898764 | pass/pass |
| sparse64 | exact | tiled | 0.980468 | 0.972584/0.988211 | 0.932461/1.084735 | 1.003439 | miss/miss |
| opaque-tiny | competitive | full | 0.962520 | 0.928684/0.997789 | 0.714431/1.202645 | 0.944134 | miss/pass |
| nested64 | competitive | full | 1.000160 | 0.996698/1.002177 | 0.964004/1.028529 | 1.031459 | miss/miss |
| opaque-clip | exact | full | 1.001758 | 0.997010/1.005277 | 0.992100/1.106075 | 1.027308 | miss/miss |
| image-only | competitive | full | 1.005906 | 0.991359/1.014001 | 0.983068/1.019960 | 1.292835 | pass/miss |
| opaque64 | competitive | full | 0.629552 | 0.624584/0.636536 | 0.571129/0.669696 | 1.096016 | pass/miss |
| tiny | competitive | cold | 1.008512 | 0.991549/1.027056 | 0.839031/1.030477 | 0.968182 | miss/pass |
| clip-churn | exact | full | 0.999817 | 0.994999/1.004113 | 0.928526/1.076272 | 1.091647 | miss/miss |
| opaque64 | competitive | tiled | 0.936188 | 0.929598/0.943612 | 0.871725/0.995106 | 0.927875 | pass/pass |
| tiny | exact | cold | 1.020569 | 1.007412/1.030459 | 0.977333/1.153342 | 1.013889 | miss/miss |
| overcapacity128 | exact | full | 0.999945 | 0.985806/1.008484 | 0.756808/1.193701 | 0.976190 | miss/pass |
| opaque-tiny | exact | full | 0.962719 | 0.952599/0.976316 | 0.922795/1.048029 | 1.037037 | miss/miss |
| opaque64 | competitive | tile | 0.964562 | 0.952255/0.976475 | 0.909229/1.047675 | 0.942857 | miss/pass |
| tiny | competitive | full | 0.995498 | 0.978937/1.005841 | 0.885995/1.063876 | 0.888889 | miss/pass |
| default-knockout64 | exact | full | 1.001803 | 0.988528/1.014033 | 0.954604/1.181195 | 1.061870 | miss/miss |
| outline2-knockout | exact | full | 1.006975 | 1.001312/1.011419 | 0.882625/1.059624 | 0.852543 | miss/pass |
| opaque64 | competitive | cold | 0.686858 | 0.678965/0.696197 | 0.487742/0.692088 | 0.834363 | pass/pass |
| one-source64 | exact | full | 0.999614 | 0.992495/1.005669 | 0.674806/1.182195 | 1.000522 | miss/miss |
| opaque-clip | competitive | full | 0.998608 | 0.996349/1.001435 | 0.962107/1.032422 | 0.955882 | miss/pass |
| outline2-late64 | competitive | full | 1.003995 | 0.996667/1.008090 | 0.978902/1.028379 | 1.012500 | miss/miss |
| outline2 | competitive | full | 1.011468 | 1.007797/1.016652 | 1.001360/1.065315 | 1.176179 | miss/miss |
| opaque-distant | exact | full | 0.999529 | 0.983156/1.010423 | 0.740678/1.023660 | 0.992188 | miss/pass |
| default-knockout64 | competitive | full | 1.002326 | 0.990760/1.011903 | 0.820872/1.064164 | 0.999264 | miss/pass |
| opaque-fill | competitive | full | 1.008686 | 0.992875/1.019887 | 0.929344/1.153257 | 0.868657 | miss/pass |
| sparse64 | competitive | tile | 0.992137 | 0.983845/1.009747 | 0.779152/1.043110 | 0.950704 | miss/pass |
| opaque-tiny | competitive | cold | 1.034964 | 1.024988/1.047345 | 0.862645/1.095017 | 1.000000 | miss/pass |
| sparse64 | competitive | full | 0.997606 | 0.988590/1.006583 | 0.913778/1.176746 | 1.144966 | miss/miss |
| outline2-knockout | competitive | full | 1.006204 | 0.997334/1.018667 | 0.879756/1.065642 | 1.322884 | miss/miss |
| opaque-tiny | exact | cold | 1.044634 | 1.030626/1.057835 | 0.969398/1.080519 | 1.000000 | miss/pass |
| opaque-empty | exact | full | 0.755112 | 0.739057/0.774238 | 0.556661/0.723196 | 0.923295 | pass/pass |
| sparse64 | exact | cold | 1.002749 | 0.995136/1.009416 | 0.729561/0.940227 | 0.929303 | pass/pass |
| opaque64 | exact | tile | 0.957165 | 0.943744/0.970739 | 0.941464/1.115623 | 1.050000 | miss/miss |
| outline2 | exact | full | 1.002693 | 0.998717/1.007080 | 0.986743/1.097771 | 1.119672 | miss/miss |
| overcapacity128 | competitive | full | 0.997304 | 0.992771/1.001796 | 0.959553/1.147569 | 1.004219 | miss/miss |
| opaque64 | exact | full | 0.706178 | 0.700776/0.710848 | 0.615729/0.743021 | 0.927885 | pass/pass |
| outline2-late64 | exact | full | 1.003739 | 0.997135/1.011601 | 0.795715/1.015905 | 0.945333 | pass/pass |
| nested64 | exact | full | 1.011010 | 0.997906/1.019238 | 0.723725/1.240424 | 0.998588 | miss/pass |

No overall-stack, latest-main, cold-I/O, global-accuracy or competitor-superiority claim. The 14 authoritative checks passed as separate stages; WASI was a bounded portability diagnostic. Original interrupted samples remain excluded. Confidence bounds are marginal and not independently recomputed.
