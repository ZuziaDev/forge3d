mod common;

use common::*;
use forge3d_core::ops;
use forge3d_core::prelude::*;
use forge3d_core::primitives as p;
use glam::Vec3;

const TOP: FaceId = FaceId(1);
const FRONT: FaceId = FaceId(2);

fn edge(m: &Mesh, a: u32, b: u32) -> EdgeId {
    m.find_edge(VertexId(a), VertexId(b)).expect("edge exists")
}

// ---------------------------------------------------------------- extrude

#[test]
fn extrude_one_cube_face() {
    let m = cube(2.0);
    let r = ops::extrude_faces(&m, &[TOP], 0.5, None).unwrap();
    assert_counts(&r.mesh, 12, 20, 10);
    assert_closed_manifold(&r.mesh);
    assert_approx(r.mesh.signed_volume(), 10.0, 1e-4);
    assert_eq!(
        r.selection.faces.iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    // moved face sits at z = 1.5
    for v in r.mesh.face_vertices(TOP) {
        assert_approx(r.mesh.position(v).z, 1.5, 1e-6);
    }
}

#[test]
fn extrude_region_does_not_duplicate_shared_edges() {
    let m = cube(2.0);
    let r = ops::extrude_faces(&m, &[TOP, FRONT], 0.5, None).unwrap();
    // 6 region vertices duplicated, 6 boundary edges -> 6 side quads.
    assert_counts(&r.mesh, 14, 24, 12);
    assert_closed_manifold(&r.mesh);
}

#[test]
fn extrude_with_offset_vector() {
    let m = cube(2.0);
    let r = ops::extrude_faces(&m, &[TOP], 0.0, Some(Vec3::new(0.0, 0.0, 2.0))).unwrap();
    assert_closed_manifold(&r.mesh);
    assert_approx(r.mesh.signed_volume(), 16.0, 1e-4);
}

#[test]
fn extrude_plane_builds_open_box_and_drops_interior_vertices() {
    let m = p::plane(2.0, 1).unwrap();
    let r = ops::extrude_faces(&m, &[FaceId(0)], 1.0, None).unwrap();
    assert_counts(&r.mesh, 8, 12, 5);
    let v = r.mesh.validate();
    assert!(v.is_valid && !v.is_closed);
    assert_eq!(v.boundary_edges, 4);

    let g = p::plane(2.0, 2).unwrap();
    let all: Vec<FaceId> = g.face_ids().collect();
    let r = ops::extrude_faces(&g, &all, 1.0, None).unwrap();
    // 9 new vertices, 8 border originals kept, the old center vertex removed.
    assert_counts(&r.mesh, 17, 28, 12);
    assert_valid(&r.mesh);
    assert_eq!(r.mesh.isolated_vertices().len(), 0);
}

// ---------------------------------------------------------------- inset

#[test]
fn inset_one_face() {
    let m = cube(2.0);
    let r = ops::inset_faces(&m, &[TOP], 0.25, 0.0, false).unwrap();
    assert_counts(&r.mesh, 12, 20, 10);
    assert_closed_manifold(&r.mesh);
    assert_approx(r.mesh.face_area(TOP), 1.5 * 1.5, 1e-4);
    assert_approx(r.mesh.signed_volume(), 8.0, 1e-4);
    // with depth the inner face is pushed out
    let r = ops::inset_faces(&m, &[TOP], 0.25, 0.5, false).unwrap();
    assert_closed_manifold(&r.mesh);
    assert!(r.mesh.signed_volume() > 8.0);
}

#[test]
fn inset_region_and_individual() {
    let m = cube(2.0);
    let region = ops::inset_faces(&m, &[TOP, FRONT], 0.2, 0.0, false).unwrap();
    assert_counts(&region.mesh, 14, 24, 12);
    assert_closed_manifold(&region.mesh);
    let ind = ops::inset_faces(&m, &[TOP, FRONT], 0.2, 0.0, true).unwrap();
    assert_counts(&ind.mesh, 16, 28, 14);
    assert_closed_manifold(&ind.mesh);
    let closed: Vec<FaceId> = m.face_ids().collect();
    assert_eq!(
        ops::inset_faces(&m, &closed, 0.2, 0.0, false)
            .unwrap_err()
            .code(),
        "invalid_selection"
    );
}

// ---------------------------------------------------------------- bevel

#[test]
fn bevel_single_cube_edge() {
    let m = cube(2.0);
    let e = edge(&m, 4, 5); // top-front edge
    let r = ops::bevel_edges(&m, &[e], 0.2).unwrap();
    assert_counts(&r.mesh, 10, 15, 7);
    assert_closed_manifold(&r.mesh);
    // removes a prism with legs 0.2 x 0.2 and length 2
    assert_approx(r.mesh.signed_volume(), 8.0 - 0.2 * 0.2, 1e-4);
    assert_eq!(r.selection.faces.len(), 1);
}

#[test]
fn bevel_all_cube_edges() {
    let m = cube(2.0);
    let all: Vec<EdgeId> = m.edge_ids().collect();
    let r = ops::bevel_edges(&m, &all, 0.1).unwrap();
    assert_counts(&r.mesh, 24, 48, 26);
    assert_closed_manifold(&r.mesh);
    let tris = r
        .mesh
        .face_ids()
        .filter(|&f| r.mesh.face_len(f) == 3)
        .count();
    assert_eq!(tris, 8);
    assert_eq!(r.selection.faces.len(), 20);
}

#[test]
fn bevel_two_adjacent_and_three_corner_edges() {
    let m = cube(2.0);
    // two edges of the top face meeting at vertex 6
    let r = ops::bevel_edges(&m, &[edge(&m, 5, 6), edge(&m, 6, 7)], 0.2).unwrap();
    assert_closed_manifold(&r.mesh);
    // three edges meeting at vertex 6 -> triangular corner patch
    let r = ops::bevel_edges(&m, &[edge(&m, 5, 6), edge(&m, 6, 7), edge(&m, 2, 6)], 0.2).unwrap();
    assert_closed_manifold(&r.mesh);
    assert!(r.mesh.face_ids().any(|f| r.mesh.face_len(f) == 3));
}

#[test]
fn bevel_on_subdivided_and_grid_meshes() {
    // valence-4 vertices on a flat grid and a cylinder ring
    let g = p::plane(2.0, 4).unwrap();
    let e = g.find_edge(VertexId(7), VertexId(12)).unwrap(); // interior edge
    let r = ops::bevel_edges(&g, &[e], 0.05).unwrap();
    assert_valid(&r.mesh);
    let c = p::cylinder(1.0, 2.0, 12).unwrap();
    let ring: Vec<EdgeId> = c
        .edge_ids()
        .filter(|&e| {
            let [a, b] = c.edge_vertices(e);
            c.position(a).z > 0.0 && c.position(b).z > 0.0
        })
        .collect();
    assert_eq!(ring.len(), 12);
    let r = ops::bevel_edges(&c, &ring, 0.1).unwrap();
    assert_closed_manifold(&r.mesh);
    assert_eq!(r.mesh.face_count(), c.face_count() + 12);
}

#[test]
fn bevel_rejects_boundary_and_large_width() {
    let g = p::plane(2.0, 1).unwrap();
    let err = ops::bevel_edges(&g, &[EdgeId(0)], 0.1).unwrap_err();
    assert_eq!(err.code(), "invalid_selection");
    let m = cube(2.0);
    assert_eq!(
        ops::bevel_edges(&m, &[EdgeId(0)], 1.5).unwrap_err().code(),
        "invalid_parameter"
    );
}

// ---------------------------------------------------------------- loop cut

#[test]
fn loop_cut_cube() {
    let m = cube(2.0);
    let e = edge(&m, 4, 5);
    let r = ops::loop_cut(&m, e, 1, 0.5).unwrap();
    assert_counts(&r.mesh, 12, 20, 10);
    assert_closed_manifold(&r.mesh);
    assert_eq!(r.selection.edges.len(), 4);
    // new vertices lie at x = 0 (midpoint of edge 4-5 along x)
    let new: Vec<Vec3> = (8..12).map(|i| r.mesh.position(VertexId(i))).collect();
    assert!(new.iter().all(|p| p.x.abs() < 1e-6));

    let r3 = ops::loop_cut(&m, e, 3, 0.5).unwrap();
    assert_counts(&r3.mesh, 20, 36, 18);
    assert_closed_manifold(&r3.mesh);

    let rf = ops::loop_cut(&m, e, 1, 0.25).unwrap();
    let xs: Vec<f32> = (8..12).map(|i| rf.mesh.position(VertexId(i)).x).collect();
    assert!(xs.iter().all(|&x| (x + 0.5).abs() < 1e-6), "{xs:?}");
}

#[test]
fn loop_cut_open_grid_and_ngon_ends() {
    let g = p::plane(3.0, 3).unwrap();
    let e = g.find_edge(VertexId(0), VertexId(1)).unwrap();
    let r = ops::loop_cut(&g, e, 1, 0.5).unwrap();
    assert_counts(&r.mesh, 20, 31, 12);
    let v = r.mesh.validate();
    assert!(v.is_valid);
    assert_eq!(v.boundary_edges, 14);

    // ring on a cylinder side quad stops at the n-gon caps, which get the new vertices
    let c = p::cylinder(1.0, 2.0, 8).unwrap();
    let e = c.find_edge(VertexId(0), VertexId(1)).unwrap(); // bottom ring edge
    let r = ops::loop_cut(&c, e, 1, 0.5).unwrap();
    assert_counts(&r.mesh, 18, 27, 11);
    assert_closed_manifold(&r.mesh);

    // around the cylinder (vertical edge): closed ring of 8 quads
    let e = c.find_edge(VertexId(0), VertexId(8)).unwrap();
    let r = ops::loop_cut(&c, e, 2, 0.5).unwrap();
    assert_counts(&r.mesh, 32, 56, 26);
    assert_closed_manifold(&r.mesh);
}

// ---------------------------------------------------------------- subdivide

#[test]
fn catmull_clark_cube() {
    let m = cube(2.0);
    let s = ops::subdivide_catmull_clark(&m, 1).unwrap();
    assert_counts(&s, 26, 48, 24);
    assert_closed_manifold(&s);
    assert!(s.face_ids().all(|f| s.face_len(f) == 4));
    // corner points of a unit-ish cube move to (±5/9) for a cube of half-size 1
    for v in 0..8 {
        let p = s.position(VertexId(v));
        for c in p.to_array() {
            assert_approx(c.abs(), 5.0 / 9.0, 1e-5);
        }
    }
    let s2 = ops::subdivide_catmull_clark(&m, 2).unwrap();
    assert_counts(&s2, 98, 192, 96);
    assert_closed_manifold(&s2);
    // the surface shrinks monotonically towards the rounded limit surface
    let (v1, v2) = (s.signed_volume(), s2.signed_volume());
    assert!(8.0 > v1 && v1 > v2 && v2 > 2.5, "{v1} {v2}");
}

#[test]
fn catmull_clark_boundaries_and_ngons() {
    let g = p::plane(2.0, 1).unwrap();
    let s = ops::subdivide_catmull_clark(&g, 1).unwrap();
    assert_counts(&s, 9, 12, 4);
    assert_valid(&s);
    // corners stay fixed, boundary edge points are midpoints
    for v in 0..4 {
        assert_eq!(s.position(VertexId(v)), g.position(VertexId(v)));
    }
    let cyl = p::cylinder(1.0, 2.0, 6).unwrap();
    let s = ops::subdivide_catmull_clark(&cyl, 1).unwrap();
    assert_eq!(s.face_count(), 6 * 4 + 2 * 6);
    assert_closed_manifold(&s);
    let t = ops::subdivide_catmull_clark(&p::torus(1.0, 0.3, 8, 6).unwrap(), 1).unwrap();
    assert_closed_manifold(&t);
    assert_eq!(t.euler_characteristic(), 0);
    assert!(ops::subdivide_catmull_clark(&m_ico(), 0).is_err());
}

fn m_ico() -> Mesh {
    p::ico_sphere(1.0, 1).unwrap()
}

// ---------------------------------------------------------------- merge

#[test]
fn merge_top_vertices_at_center() {
    let m = cube(2.0);
    let top: Vec<VertexId> = (4..8).map(VertexId).collect();
    let r = ops::merge_at_center(&m, &top).unwrap();
    assert_counts(&r.mesh, 5, 8, 5);
    assert_closed_manifold(&r.mesh);
    let apex = *r.selection.vertices.iter().next().unwrap();
    assert!(r
        .mesh
        .position(VertexId(apex))
        .abs_diff_eq(Vec3::new(0.0, 0.0, 1.0), 1e-6));
}

#[test]
fn merge_by_distance_welds_split_cube() {
    let m = cube(2.0);
    // explode: every face gets its own vertices
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    for f in m.face_ids() {
        let mut face = Vec::new();
        for v in m.face_vertices(f) {
            pos.push(m.position(v) + Vec3::splat(1e-6));
            face.push((pos.len() - 1) as u32);
        }
        faces.push(face);
    }
    let soup = Mesh::from_polygons(pos, &faces).unwrap();
    assert_eq!(soup.vertex_count(), 24);
    assert_eq!(soup.validate().boundary_edges, 24);
    let (r, removed) = ops::merge_by_distance(&soup, None, 1e-4).unwrap();
    assert_eq!(removed, 16);
    assert_counts(&r.mesh, 8, 12, 6);
    assert_closed_manifold(&r.mesh);
}

// ---------------------------------------------------------------- delete

#[test]
fn delete_elements() {
    let m = cube(2.0);
    let f = ops::delete_elements(&m, ElementKind::Face, &[1]).unwrap();
    assert_counts(&f, 8, 12, 5);
    assert_eq!(f.validate().boundary_edges, 4);
    assert_valid(&f);

    let v = ops::delete_elements(&m, ElementKind::Vertex, &[6]).unwrap();
    assert_counts(&v, 7, 9, 3);
    assert_valid(&v);

    let e = ops::delete_elements(&m, ElementKind::Edge, &[edge(&m, 4, 5).0]).unwrap();
    assert_counts(&e, 8, 11, 4);
    assert_valid(&e);

    // deleting all faces leaves nothing
    let all = ops::delete_elements(&m, ElementKind::Face, &[0, 1, 2, 3, 4, 5]).unwrap();
    assert!(all.is_empty());
    assert!(ops::delete_elements(&m, ElementKind::Face, &[42]).is_err());
}

#[test]
fn mesh_removal_api_compacts() {
    let mut m = cube(2.0);
    let remap = m.remove_faces(&[FaceId(0)], false).unwrap();
    assert_eq!(remap.faces[0], u32::MAX);
    assert_eq!(remap.faces[1], 0);
    assert_eq!(m.face_count(), 5);
    let mut m = cube(2.0);
    m.remove_vertices(&[VertexId(0)]).unwrap();
    assert_counts(&m, 7, 9, 3);
    assert!(m.validate().dangling_references == 0);
}

// ---------------------------------------------------------------- mirror

#[test]
fn mirror_without_merge_duplicates() {
    let m = translated(cube(2.0), Vec3::new(2.0, 0.0, 0.0));
    let r = ops::mirror(&m, Axis::X, false, 1e-4).unwrap();
    assert_counts(&r, 16, 24, 12);
    assert_closed_manifold(&r);
    assert_eq!(r.face_components().len(), 2);
    assert_approx(r.signed_volume(), 16.0, 1e-3);
}

#[test]
fn mirror_with_merge_joins_halves() {
    // half box: x in [0, 2], open at x = 0
    let m = translated(cube(2.0), Vec3::new(1.0, 0.0, 0.0));
    let open = ops::delete_elements(&m, ElementKind::Face, &[4]).unwrap(); // -X face
    let r = ops::mirror(&open, Axis::X, true, 1e-4).unwrap();
    assert_counts(&r, 12, 20, 10);
    assert_closed_manifold(&r);
    assert_approx(r.signed_volume(), 16.0, 1e-3);

    // closed half touching the plane: internal faces cancel
    let r = ops::mirror(&m, Axis::X, true, 1e-4).unwrap();
    assert_closed_manifold(&r);
    assert_counts(&r, 12, 20, 10);
}

// ---------------------------------------------------------------- normals

#[test]
fn recalculate_normals_fixes_flipped_faces() {
    let mut m = cube(2.0);
    m.flip_faces(&[FaceId(1), FaceId(3)]);
    let r = m.validate();
    assert!(!r.is_consistently_oriented);
    assert!(r.inconsistent_winding_edges > 0);
    let fixed = ops::recalculate_normals(&m, false).unwrap();
    assert_closed_manifold(&fixed);
    assert_outward_convex(&fixed);

    let mut all_in = cube(2.0);
    all_in.flip_all_faces();
    assert!(all_in.signed_volume() < 0.0);
    let fixed = ops::recalculate_normals(&all_in, false).unwrap();
    assert!(fixed.signed_volume() > 0.0);
    let inward = ops::recalculate_normals(&fixed, true).unwrap();
    assert!(inward.signed_volume() < 0.0);
    assert!(inward.validate().is_valid);

    let flipped = ops::flip_normals(&fixed, &[FaceId(0)]).unwrap();
    assert!(!flipped.validate().is_consistently_oriented);
}

// ---------------------------------------------------------------- misc mesh

#[test]
fn ngon_mesh_roundtrip_and_triangulation() {
    // L-shaped prism with concave n-gon caps
    let outline = [
        (0.0, 0.0),
        (2.0, 0.0),
        (2.0, 1.0),
        (1.0, 1.0),
        (1.0, 2.0),
        (0.0, 2.0),
    ];
    let n = outline.len() as u32;
    let mut pos = Vec::new();
    for z in [0.0, 1.0] {
        for &(x, y) in &outline {
            pos.push(Vec3::new(x, y, z));
        }
    }
    let mut faces: Vec<Vec<u32>> = vec![(0..n).rev().collect(), (n..2 * n).collect()];
    for i in 0..n {
        let j = (i + 1) % n;
        faces.push(vec![i, j, j + n, i + n]);
    }
    let m = Mesh::from_polygons(pos.clone(), &faces).unwrap();
    assert_closed_manifold(&m);
    assert_approx(m.signed_volume(), 3.0, 1e-4);
    let tris = m.triangulate();
    assert_eq!(tris.len(), 2 * 4 + 6 * 2);
    let (p2, f2) = m.to_polygons();
    assert_eq!(p2, pos);
    assert_eq!(f2, faces);
    // serde roundtrip through MeshData
    let json = serde_json::to_string(&m).unwrap();
    let back: Mesh = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
}

#[test]
fn from_polygons_rejects_bad_input() {
    let empty = Mesh::new();
    assert_eq!(empty.validate().dangling_references, 0);
    assert_eq!(Mesh::default(), empty);
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
    assert_eq!(
        Mesh::from_polygons(pos.clone(), &[vec![0, 1]])
            .unwrap_err()
            .code(),
        "invalid_mesh"
    );
    assert!(Mesh::from_polygons(pos.clone(), &[vec![0, 1, 5]]).is_err());
    assert!(Mesh::from_polygons(pos.clone(), &[vec![0, 1, 1]]).is_err());
    let cleaned = Mesh::from_polygons_cleaned(pos, vec![vec![0, 1, 1, 2], vec![0, 0, 1]]).unwrap();
    assert_eq!(cleaned.face_count(), 1);
}

#[test]
fn adjacency_queries() {
    let m = cube(2.0);
    for v in m.vertex_ids() {
        assert_eq!(m.vertex_valence(v), 3);
        assert_eq!(m.vertex_faces(v).len(), 3);
        assert!(!m.is_boundary_vertex(v));
    }
    for e in m.edge_ids() {
        assert_eq!(m.edge_faces(e).len(), 2);
        for l in m.edge_loops(e) {
            let t = m.loop_twin(l).unwrap();
            assert_eq!(m.loop_vertex(t), m.loop_target(l));
            assert_eq!(m.loop_twin(t), Some(l));
        }
    }
    assert_eq!(m.face_neighbors(TOP).len(), 4);
    // edge ring around a cube: 4 parallel edges
    assert_eq!(m.edge_ring(edge(&m, 4, 5)).len(), 4);
    // torus: closed loops along both directions
    let t = p::torus(1.0, 0.25, 12, 8).unwrap();
    let e = t.find_edge(VertexId(0), VertexId(8)).unwrap(); // along major direction
    assert_eq!(t.edge_loop(e).len(), 12);
    assert_eq!(t.edge_ring(e).len(), 8);
    let e = t.find_edge(VertexId(0), VertexId(1)).unwrap(); // along minor direction
    assert_eq!(t.edge_loop(e).len(), 8);
    assert_eq!(t.edge_ring(e).len(), 12);
    // grid: open loop across
    let g = p::plane(3.0, 3).unwrap();
    let e = g.find_edge(VertexId(5), VertexId(6)).unwrap();
    assert_eq!(g.edge_loop(e).len(), 3);
    let b = g.bounding_box().unwrap();
    assert!(b.min.abs_diff_eq(Vec3::new(-1.5, -1.5, 0.0), 1e-6));
}

#[test]
fn validate_detects_non_manifold() {
    // three triangles sharing one edge
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::NEG_Y];
    let m = Mesh::from_polygons(pos, &[vec![0, 1, 2], vec![1, 0, 3], vec![1, 0, 4]]).unwrap();
    let r = m.validate();
    assert_eq!(r.non_manifold_edges, 1);
    assert!(!r.is_manifold && !r.is_valid);

    // bow-tie: two triangles sharing only a vertex
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::NEG_X, Vec3::NEG_Y];
    let m = Mesh::from_polygons(pos, &[vec![0, 1, 2], vec![0, 3, 4]]).unwrap();
    let r = m.validate();
    assert_eq!(r.non_manifold_vertices, 1);

    // degenerate face
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
    let m = Mesh::from_polygons(pos, &[vec![0, 1, 2]]).unwrap();
    assert_eq!(m.validate().degenerate_faces, 1);
}

#[test]
fn large_mesh_performance() {
    // ~82k triangles: build, validate, normals, subdivide once
    let m = p::ico_sphere(1.0, 6).unwrap();
    assert_eq!(m.face_count(), 81920);
    let r = m.validate();
    assert!(r.is_closed && r.is_valid);
    let s = ops::subdivide_catmull_clark(&p::ico_sphere(1.0, 5).unwrap(), 1).unwrap();
    assert_eq!(s.face_count(), 20480 * 3);
    assert!(s.is_closed());
    let top = forge3d_core::selection::elements_facing(&m, ElementKind::Face, Vec3::Z, 10.0, None)
        .unwrap();
    let ex = ops::extrude_faces(
        &m,
        &top.iter().map(|&f| FaceId(f)).collect::<Vec<_>>(),
        0.1,
        None,
    )
    .unwrap();
    assert!(ex.mesh.validate().is_closed);
}
