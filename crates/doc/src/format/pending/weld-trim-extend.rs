//! A weldment Trim/Extend feature (`weld_trim`): structural member bodies (`members`) whose ends
//! nearest a boundary are cut back or lengthened to it, the boundary being a plane (`plane`), a
//! face of a body (`face`: body and a point on it; planar, or round when coped) or other members
//! (`to`), with which way they may move (`mode`: trim or extend, saved only when not both), the gap
//! left at each end (`gap`) and, with members or a round face as the boundary, whether each end is
//! coped to fit its outside (`cope`, saved only when true). Older designs need no transformation:
//! they have no trim/extend features. The last release asks for an update instead of opening a
//! design with this format number.
