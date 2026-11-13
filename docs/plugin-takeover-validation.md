# FigCraft recovery validation — 2026-11-13

Base: `9e33380f`. Recovered changes wake the UI through the worker bridge, distinguish document-content changes from selection/view changes and rollback, prevent comment-author spoofing, and permit large live IDs while bounding numeric dimensions.

The first adapter run reported 25 passed, 1 failed and 2 ignored. The failing wake-up test bypassed the real bridge in its recording worker. The fixture now uses the bridge. A subsequent compile caught an invalid AppContract clone in that fixture; it was replaced with contract construction. These latest corrections and the ignored package cases have **no confirmed passing rerun** following the tool outage.

Large unsigned integers are exempt from numeric coordinate limits only in named identifier fields. Review that identifier-key list against canonical engine command schemas before expanding it. Current shared renderer/UI integration also needs source review.

No new builds or suites are permitted during handoff. Use source review and only cheap scoped checks; defer app/CLI builds, real-package tests, desktop click-through and platform gates until separately authorized. Known limitations remain in plugin-migration.md; notably FigCraft has no raster.filter dispatch and must not advertise it.
