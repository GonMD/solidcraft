//! A weldment Structural Member feature (`weld_member`): a closed profile of one sketch
//! (`sketch`, `profiles`) along lines and arcs of another (`path_sketch`, `path`), with the
//! profile's turn about each line (`angle`), how members meeting at their ends are joined
//! (`corner`: mitre or butt), joints joined otherwise (`joints`: two members, a corner and for a
//! butt the member running through), the gap at butt joints (`gap`) and whether members ending on
//! an earlier Structural Member's members butt against them (`join_earlier`, saved only when
//! false). Further groups of lines (`groups`: each a `path_sketch` and `path`, with its own
//! `angle` and `corner` where given) join the `path` as one; a member is keyed in `joints` by its
//! line's id, `N:line` for a line of group `N` (from 2). Its `body_names` follow its members in
//! the order of their keys (`path`, then each group's lines): each member's name belongs to its
//! skeleton line.
//! An arc makes a bent member, a spline, conic or other free-form curve or a 3D sketch curve one
//! formed to its shape. With `merge_arcs` (saved only when false), lines and curves that run on
//! into one another (tangent, no other listed curve ending there) make one bent member, keyed by
//! the first of them listed, and a ring where they close on themselves (as a closed curve alone
//! does); `end_allowance` (saved where given) is the stock added at each end of a bent member's
//! cut length. The evaluated model a design saves holds each bent member's centreline, bends,
//! length past each end and profile frame at its end (`bent`), and whether it is formed to a
//! free-form shape (`formed`, saved only when true).
//! Which point of the profile lies on each line is `locate` (saved unless the centroid: one of the
//! nine points of the profile's bounding box, `top_left`…`bottom_right` with `middle`, or
//! `{point: [x, y]}` in the profile's coordinates), and `mirror` (`left_right` or `up_down`, saved
//! unless none) mirrors the profile about the axes through that point; a group may give its own
//! `locate` and `mirror` (`none` there undoes the feature's).
//! Instead of `sketch` and `profiles`, the profile may be `stock`: a profile from the weldment
//! profile library, copied in as `{name, region}` (the region's outer loop and holes, as segments)
//! so the design builds without the library, with the library's cut list `description` and free
//! `properties` where it gives them.
//! Older designs need no transformation: they have no structural members. The last release asks
//! for an update instead of opening a design with this format number.
