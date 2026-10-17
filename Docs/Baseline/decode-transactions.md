# Prepared RAW samples

E3 retains one decoded sensor allocation for the active photo until its first full development. It does not retain a second RGB original. Nikon's sample-reading import probe transfers its samples to the preview; header-only probes let the first preview populate the same owner. A content digest and decoder version validate reuse. The normal filesystem hooks remain available to other engine clients.

The owner is capped at 128 MiB, including sample capacity and serialized metadata. Metadata must round-trip exactly and fit within 8 MiB. Only one active preparation exists; a cancelled or obsolete loader cannot restore it. An unconfirmed import expires after five seconds. Confirmed selection has no idle timeout, so a first zoom after a long fit inspection still reuses samples. First full development releases samples before color/resize scratch; replacement, reload, suspend and pressure also clear them.

A first zoom still performs full demosaic, color processing and GPU upload. E3 removes repeated entropy decoding; it does not make those other phases free. After deliberate E1 Full-source eviction, a later zoom can decode again. A memory-pressure or oversized-source miss is also a legitimate cold path.

## Physical iPad comparison

Measured on 2026-10-15, iPad Air 11-inch M2, iPadOS 27.0 (24A437), Release. The same frozen E1/P1/E5 executable ran both paths: `UNVEIL_E3_PREPARED=on` uses the 128 MiB owner; `off` sets its capacity to zero. The latter preserves the same loader and diagnostic counters but cannot reuse sensor samples. This switch exists only in the isolated acceptance copy, not production.

Three runs per RAW/path used a fresh temporary catalog, a 15-second fit hold and first 100% zoom; each cold run was followed by a warm-catalog fresh-process open. There were 24 launches. Cold trials completed at nominal thermal state. The warm trials capture the open timeline without a separate thermal snapshot. Run order was on/off, off/on, on/off. Source, library hash, harness and logs are in `.superpowers/sdd/2026-10-12-m6/acceptance-control/`, `e3-control-library.sha256` and `e3-measurements/`.

| Median milliseconds, 3 runs | Capacity zero | Prepared samples | Difference |
|---|---:|---:|---:|
| RAF cold first Full presented | 671.73 | 665.40 | -6.33 |
| RAF warm first Full presented | 635.43 | 618.20 | -17.23 |
| RAF first 100% Full window delivered | 1064.94 | 783.32 | -281.62 |
| NEF cold first Full presented | 960.11 | 746.82 | -213.29 |
| NEF warm first Full presented | 629.75 | 634.93 | +5.18 |
| NEF first 100% Full window delivered | 1721.76 | 1612.59 | -109.17 |

Open and zoom use different endpoints: open ends at drawable presentation; zoom ends when the expected Full window reaches the frame sink. The table does not equate delivery with presentation. NEF zoom timings vary considerably: prepared 1268–1773 ms versus control 1607–1911 ms. One paired run was 6 ms slower; the median improvement is not a uniform per-run saving. One warm prepared NEF open took 720 ms; the other two were 635 and 631 ms. No warm-NEF speedup is claimed.

The NEF preview loader drops from a median 457.54 to 200.13 ms after the import probe passes its samples forward. That saved loader work supports the 213 ms cold-open improvement and passes the proposed 150 ms median gate. RAF preview loader work is essentially the same operation; its benefit is first zoom. Physical sample counters are one versus two decodes through first full zoom for RAF, and one versus three for NEF. Prepared hits are one RAF and two NEF (probe-to-preview plus preview-to-full). The prepared owner is empty after full development.

## Memory tradeoff

At the 15-second fit hold, sensor storage is exactly 53,397,637 bytes for RAF and 91,501,347 bytes for NEF. No second sample Vec or full RGB allocation belongs to that owner.

| Fit footprint median, MiB | Capacity zero | Prepared samples |
|---|---:|---:|
| RAF | 463.9 | 515.1 |
| NEF | 481.3 | 568.7 |

The additional 51.2/87.4 MiB is the intended sensor retention. Lifetime peak medians are 2307.9/2262.8 MiB (control/prepared) for RAF and 3743.5/3746.4 MiB for NEF. Peak decode/development memory is not solved by reuse. The short, two-second post-zoom footprint varies widely even with identical live source ownership; use the six longer E1 runs for fit-after-release behavior. E9 and E10 separately track allocator/peak and GPU pool work.

## Correctness and remaining limits

Host fixtures prove one real sample-dispatch call per eligible open/first zoom, exact prepared/ordinary RGB and metadata for the five RAWs, bounded ownership, pressure/cancellation, content mismatch and concurrent leases. The frozen E1/E3/E5 Mac candidate produced all 25 Full images exactly equal to existing Mac goldens. Existing references were preserved.

Actual pressure/oversize misses can decode again. The hardware paired run exercises normal eligible input, not every pressure or corrupt-file path. Detailed phase timing is the whole loader transaction; it does not separately time sensor decode versus demosaic, and the decoder counters must not be relabeled as timing samples. Final combined E2/E4/E5 output and interaction acceptance is recorded separately.
