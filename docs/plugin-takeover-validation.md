# FigCraft recovery validation — 2026-11-13

Base: `9e33380f`. Recovered changes wake the UI through the worker bridge, distinguish document-content changes from selection/view changes and rollback, prevent comment-author spoofing, and permit large live IDs while bounding numeric dimensions.

The first adapter run reported 25 passed, 1 failed and 2 ignored. The failing wake-up test bypassed the real bridge in its recording worker. The fixture now uses the bridge. A subsequent compile caught an invalid AppContract clone in that fixture; it was replaced with contract construction. These latest corrections and the ignored package cases have **no confirmed passing rerun** following the tool outage.

Large unsigned integers are exempt from numeric coordinate limits only in named identifier fields. Review that identifier-key list against canonical engine command schemas before expanding it. Current shared renderer/UI integration also needs source review.

No new builds or suites are permitted during handoff. Use source review and only cheap scoped checks; defer app/CLI builds, real-package tests, desktop click-through and platform gates until separately authorized. Known limitations remain in plugin-migration.md; notably FigCraft has no raster.filter dispatch and must not advertise it.

## Mac source review (2026-11-13)

A bounded independent read-only review traced the corrected fixture through Bridge
and its wake-after-enqueue path. Content notifications compare document Arc identity;
rollback restores the captured Arc. Selection/view changes do not mutate document
content. Numeric exceptions remain restricted to unsigned values in named identifier
fields; coordinate/dimension regressions remain in source. No further definite
source defect was found in this review. No Cargo, tests, app or UI checks ran;
the final corrected-fixture runtime result remains pending. FigCraft still provides
no raster.filter dispatch.
