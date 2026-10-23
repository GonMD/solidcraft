//! A weldment Gusset feature (`weld_gusset`): a plate in the corner between two structural members
//! (`members`, their bodies), with its leg lengths (`size`, and `size2` along the second member
//! when it differs), thickness (`thickness`), how far its sides stand square off the members before
//! its slanted edge (`edge`) and how far its corner is cut back for the weld (`clip`), how far it
//! is moved off the plane of the two lines (`offset`), which side of that plane its thickness lies
//! on (`side`: `one` or `other`, saved only when not `center`) and whether each leg runs the other
//! way along its member (`reverse_first`, `reverse_second`, saved only when true). Older designs
//! need no transformation: they have no gussets. The last release asks for an update instead of
//! opening a design with this format number.
