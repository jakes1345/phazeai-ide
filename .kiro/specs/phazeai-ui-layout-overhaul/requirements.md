# Requirements Document

## Introduction

The PhazeAI IDE UI Layout Overhaul addresses a set of concrete layout and visual defects in the Floem-based IDE. The current implementation wraps the entire window in 16px of outer padding, making the IDE look like a floating box rather than a full-window application. Panel widths are inconsistent (left panel snaps between 260px and 300px depending on how it was opened), the chat panel has no fixed width, the bottom panel has no sensible default height, and the glassmorphism aesthetic feels accidental rather than intentional. This overhaul fixes all of these issues while staying entirely within Floem and without breaking any existing functionality.

## Glossary

- **IDE_Root**: The top-level Floem view returned by `ide_root()` in `app.rs`, which contains the full IDE layout.
- **IDE_Shell**: The `ide_with_menu` stack in `launch_phaze_ide()` that wraps the menu bar and IDE root.
- **Activity_Bar**: The narrow vertical icon strip on the far left, currently 48px wide, built by `activity_bar()`.
- **Left_Panel**: The collapsible sidebar panel built by `left_panel()`, showing explorer, search, git, etc.
- **Editor_Area**: The central region containing the primary editor pane and any split panes.
- **Chat_Panel**: The right-side AI chat panel, currently unsized (takes remaining flex space).
- **Bottom_Panel**: The collapsible panel at the bottom built by `bottom_panel()`, containing terminal, problems, output, etc.
- **Menu_Bar**: The custom in-app menu bar built by `menu_bar()`, currently 24px tall.
- **Status_Bar**: The bottom status strip built by `status_bar()`, currently 24px tall.
- **Glass_Panel**: A semi-transparent panel using `glass_bg` fill and `glass_border` border to simulate frosted glass, since Floem does not expose backdrop-filter blur.
- **Glow**: A `box_shadow` applied to panel edges to create a soft luminous border effect.
- **Cosmic_Canvas**: The animated hex-grid background rendered by `cosmic_bg_canvas()`, visible only on cosmic themes.
- **PhazeTheme**: The reactive theme signal carrying the active `PhazePalette`.
- **PhazePalette**: The struct in `theme.rs` holding all color values including `glass_bg`, `glass_border`, and `glow`.

---

## Requirements

### Requirement 1: Remove Outer Window Padding

**User Story:** As a developer using PhazeAI IDE, I want the IDE to fill the entire window without any outer gap, so that it feels like a native full-window application rather than a floating box.

#### Acceptance Criteria

1. THE IDE_Shell SHALL have zero padding on all sides (no `padding(16.0)` or equivalent).
2. WHEN the IDE window is opened, THE IDE_Root SHALL extend to all four edges of the window with no visible gap between the IDE chrome and the OS window border.
3. THE IDE_Shell SHALL use `width_full()` and `height_full()` without any inset that reduces the usable area.

---

### Requirement 2: Consistent Left Panel Width

**User Story:** As a developer, I want the left panel to open to the same width regardless of how it was triggered, so that the layout does not shift unexpectedly.

#### Acceptance Criteria

1. WHEN the Left_Panel is opened by clicking an Activity_Bar button, THE Left_Panel SHALL render at exactly 260px wide.
2. WHEN the Left_Panel is opened by any other trigger (keyboard shortcut, menu action, file open event), THE Left_Panel SHALL render at exactly 260px wide.
3. THE Left_Panel SHALL NOT render at 300px under any code path.
4. WHEN the Left_Panel is closed, THE Left_Panel SHALL have a width of 0px and SHALL NOT be visible.

---

### Requirement 3: Fixed Chat Panel Width

**User Story:** As a developer, I want the AI chat panel to have a fixed, predictable width, so that the editor area does not become too narrow or too wide depending on window size.

#### Acceptance Criteria

1. WHEN the Chat_Panel is visible, THE Chat_Panel SHALL render at a fixed width of 320px.
2. THE Chat_Panel SHALL NOT grow or shrink with the window width (no `flex_grow` on the chat container).
3. WHEN the Chat_Panel is hidden, THE Chat_Panel SHALL have a width of 0px and SHALL NOT consume layout space.

---

### Requirement 4: Bottom Panel Default Height

**User Story:** As a developer, I want the bottom panel to open at a sensible default height, so that I can see terminal output without it being too small or dominating the editor.

#### Acceptance Criteria

1. WHEN the Bottom_Panel is visible and not maximized, THE Bottom_Panel SHALL render at a default height of 220px.
2. WHEN the Bottom_Panel is maximized, THE Bottom_Panel SHALL expand to fill all available vertical space below the Menu_Bar.
3. WHEN the Bottom_Panel is hidden, THE Bottom_Panel SHALL have a height of 0px and SHALL NOT consume layout space.

---

### Requirement 5: Intentional Glassmorphism on Panels

**User Story:** As a developer, I want the glass panels to feel like distinct frosted-glass layers rather than plain colored boxes, so that the cosmic aesthetic is visually coherent.

#### Acceptance Criteria

1. THE Left_Panel SHALL apply a `box_shadow` Glow on its right edge with a blur radius of at least 16px using the theme's `glow` color.
2. THE Chat_Panel SHALL apply a `box_shadow` Glow on its left edge with a blur radius of at least 16px using the theme's `glow` color.
3. THE Bottom_Panel SHALL apply a `box_shadow` Glow on its top edge with a blur radius of at least 16px using the theme's `glow` color.
4. THE Activity_Bar SHALL apply a `box_shadow` Glow on its right edge with a blur radius of at least 12px using the theme's `glow` color.
5. WHERE the active theme is a cosmic theme (MidnightBlue, Cyberpunk, Synthwave84, Andromeda), THE Glass_Panel `glass_bg` alpha SHALL be no greater than 180 (out of 255) to allow the Cosmic_Canvas to show through.
6. WHERE the active theme is a non-cosmic theme, THE Glass_Panel `glass_bg` SHALL use the theme's `bg_panel` color at full opacity.

---

### Requirement 6: Cosmic Canvas Visibility

**User Story:** As a developer using a cosmic theme, I want the animated hex-grid background to be clearly visible through the glass panels, so that the space aesthetic is actually perceptible.

#### Acceptance Criteria

1. WHERE the active theme is a cosmic theme, THE Cosmic_Canvas hex grid accent opacity SHALL be at least 18% (alpha ≥ 46 out of 255).
2. WHERE the active theme is a cosmic theme, THE Cosmic_Canvas SHALL render at full window size with no clipping from outer padding.
3. WHERE the active theme is a non-cosmic theme, THE Cosmic_Canvas SHALL NOT be rendered.

---

### Requirement 7: Activity Bar Sizing

**User Story:** As a developer, I want the activity bar to have a consistent, compact width that does not waste horizontal space.

#### Acceptance Criteria

1. THE Activity_Bar SHALL have a fixed width of 48px.
2. THE Activity_Bar icon buttons SHALL each be 40px × 40px with 4px vertical margin between them.
3. THE Activity_Bar SHALL fill the full height of the IDE_Root (from below the Menu_Bar to above the Status_Bar).

---

### Requirement 8: Menu Bar and Status Bar Heights

**User Story:** As a developer, I want the menu bar and status bar to have compact, consistent heights that do not waste vertical space.

#### Acceptance Criteria

1. THE Menu_Bar SHALL have a fixed height of 28px.
2. THE Status_Bar SHALL have a fixed height of 22px.
3. THE Menu_Bar SHALL span the full width of the IDE_Shell.
4. THE Status_Bar SHALL span the full width of the IDE_Root.

---

### Requirement 9: No Functional Regression

**User Story:** As a developer, I want all existing IDE functionality to continue working after the layout overhaul, so that I do not lose any features.

#### Acceptance Criteria

1. WHEN the layout changes are applied, THE IDE_Root SHALL continue to render the editor, LSP diagnostics, terminal, chat, git, and all other panels without error.
2. WHEN the Left_Panel resize drag handle is used, THE Left_Panel SHALL resize correctly and the drag interaction SHALL remain functional.
3. WHEN zen mode is activated, THE Activity_Bar, Left_Panel, Bottom_Panel, and Status_Bar SHALL all be hidden as before.
4. WHEN the split editor is activated, THE Editor_Area SHALL continue to display side-by-side and top-bottom split panes correctly.
5. IF a panel visibility signal changes, THEN THE IDE_Root SHALL re-render the affected panel without affecting other panels.
