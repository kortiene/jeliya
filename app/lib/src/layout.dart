/// The ONE form-factor seam: a width breakpoint, never platform,
/// orientation, or shortestSide — a tablet (or a wide desktop window) keeps
/// the three-pane shell; a narrow window gets the bottom-tab mobile shell
/// (issue #17; web parity: ui/src/styles.css `@media (max-width: 900px)`).
/// Every width fork in the app must go through [kShellBreakpoint] /
/// [isMobileWidth] so the whole UI flips together.
library;

import 'package:flutter/widgets.dart';

/// Logical px below which the shell renders the mobile bottom-tab layout.
const double kShellBreakpoint = 900;

/// True when the window is narrower than [kShellBreakpoint]. MediaQuery-based
/// (the WINDOW width, not the local pane width) so an over-painting surface
/// like the fleet overlay forks the same way as the shell hosting it, and so
/// a live window resize re-routes on the next build.
bool isMobileWidth(BuildContext context) =>
    MediaQuery.sizeOf(context).width < kShellBreakpoint;
