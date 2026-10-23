//! A weldment Weld Bead feature (`weld_bead`): a cosmetic weld along the seams where two bodies
//! meet (`bodies`), its size (`size`: a fillet's leg or a groove weld's throat), its kind (`kind`:
//! `groove`, saved only when not `fillet`), the points picking which seams it welds (`at`, saved
//! only when some are picked), the root gap the bodies may stand apart by (`gap`), intermittent
//! welds' length and pitch (`length`, `pitch`), whether it is welded on site (`field`, saved only
//! when true) and a note for the weld symbol's tail (`note`). Older designs need no
//! transformation: they have no weld beads. The last release asks for an update instead of
//! opening a design with this format number.
