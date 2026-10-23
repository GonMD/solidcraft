//! A weldment End Cap feature (`weld_end_cap`): a plate on each picked structural member end
//! (`ends`: the member's body and `start` or `end` of its skeleton line), its thickness
//! (`thickness`), how far its edge is set in from the profile's outline (`inset`), whether it is
//! fitted into the profile's hole instead of standing on the end (`inside`, saved only when true)
//! and how its sharp outside corners are finished (`corner`: `chamfer` or `fillet`, saved only
//! when not `square`, by `corner_size`). Older designs need no transformation: they have no end
//! caps. The last release asks for an update instead of opening a design with this format number.
