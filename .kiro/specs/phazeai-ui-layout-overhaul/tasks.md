# Implementation Plan: PhazeAI UI Layout Overhaul

## Overview

Surgical edits to `app.rs`, `theme.rs`, and optionally `constants.rs`. Each task targets a single change from the design document.

## Tasks

- [x] 1. Remove outer window padding from `ide_with_menu`
  - In `launch_phaze_ide()` (~line 6022), remove `.padding(16.0)` from the `ide_with_menu` stack style
  - Result: `.style(|s| s.flex_col().width_full().height_full())`
  - _Requirements: 1.1, 1.2, 1.3_

- [x] 2. Fix left panel open width in activity bar click handler
  - In `activity_bar_btn()` (~line 2149), change `state.left_panel_width.set(300.0)` to `state.left_panel_width.set(260.0)`
  - _Requirements: 2.1, 2.2, 2.3_

- [x] 3. Resize activity bar buttons
  - In `activity_bar_btn()` (~line 2130), change `s.width(42.0).height(42.0)` to `s.width(40.0).height(40.0)` and `.margin_bottom(6.0)` to `.margin_bottom(4.0)`
  - _Requirements: 7.2_

- [x] 4. Add fixed width to chat panel
  - In `ide_root()` (~line 5460), add `.width(320.0)` to the `chat_wrap` container style chain
  - _Requirements: 3.1, 3.2_

- [x] 5. Add default height to bottom panel
  - In `ide_root()` (~line 5603), in the `else` branch of the `bottom_panel_max` check, change `s` to `s.height(220.0)`
  - _Requirements: 4.1, 4.2_

- [x] 6. Increase left panel glow blur radius
  - In `left_panel()` (~line 2481), change `box_shadow_blur(12.0)` to `box_shadow_blur(16.0)`
  - _Requirements: 5.1_

- [x] 7. Increase chat panel glow blur radius
  - In the `chat_wrap` style (~line 5460), change `box_shadow_blur(12.0)` to `box_shadow_blur(16.0)`
  - _Requirements: 5.2_

- [x] 8. Update menu bar height
  - In `menu_bar()` (~line 5950), change `s.height(24.0).min_height(24.0)` to `s.height(28.0).min_height(28.0)`
  - _Requirements: 8.1, 8.3_

- [x] 9. Update status bar height
  - In `status_bar()` (~line 2941), change `s.height(24.0)` to `s.height(22.0)`
  - _Requirements: 8.2, 8.4_

- [x] 10. Increase cosmic canvas hex grid opacity
  - In `cosmic_bg_canvas()` (~line 2078), change `p.accent.with_alpha(0.09)` to `p.accent.with_alpha(0.18)`
  - _Requirements: 6.1_

- [x] 11. Sync constants with new layout values
  - In `crates/phazeai-core/src/constants.rs`, update `STATUS_BAR_HEIGHT` to `22.0`, `MENU_BAR_HEIGHT` to `28.0`, confirm `DEFAULT_CHAT_WIDTH` is `320.0`
  - _Requirements: 8.1, 8.2_

- [x] 12. Write property-based tests
  - [x] 12.1 Write property test for left panel open width (Property 1)
    - Generate random `Tab` variants, simulate click handler logic, assert `left_panel_width == 260.0`
    - Tag: `// Feature: phazeai-ui-layout-overhaul, Property 1: left panel open width is always 260px`
    - _Requirements: 2.1, 2.2, 2.3_
  - [ ]* 12.2 Write property test for cosmic theme glass_bg alpha (Property 2)
    - Iterate all `ThemeVariant` values where `is_cosmic()` is true, assert `glass_bg` alpha <= 180
    - Tag: `// Feature: phazeai-ui-layout-overhaul, Property 2: cosmic theme glass_bg alpha <= 180`
    - _Requirements: 5.5_
  - [ ]* 12.3 Write property test for non-cosmic theme glass_bg opacity (Property 3)
    - Iterate all `ThemeVariant` values where `is_cosmic()` is false, assert `glass_bg` alpha > 200
    - Tag: `// Feature: phazeai-ui-layout-overhaul, Property 3: non-cosmic theme glass_bg is opaque`
    - _Requirements: 5.6_
  - [ ]* 12.4 Write property test for zen mode panel visibility (Property 4)
    - Generate random panel visibility combinations with `zen_mode = true`, assert activity bar, left panel, bottom panel, and status bar all have `display(None)`
    - Tag: `// Feature: phazeai-ui-layout-overhaul, Property 4: zen mode hides all peripheral panels`
    - _Requirements: 9.3_

- [x] 13. Build and verify
  - Run `cargo build -p phazeai-ui` and confirm zero errors and zero new warnings
  - Run `cargo test -p phazeai-ui` and confirm all tests pass
  - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5_

## Notes

- Tasks marked with `*` are optional and can be skipped for a faster pass
- Each task is a single targeted edit — do not combine changes across tasks
- Property tests use the `proptest` crate; no new dependencies needed for pure logic tests
- Minimum 100 iterations per property test
