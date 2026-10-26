// Derived from meshoptimizer 1.3, Copyright (c) 2016-2026 Arseny Kapoulkine.
// MIT notice: ../vendor/LICENSE. Only the first pristine ranking is reused.
namespace ara_lod {
static size_t run(State& p, unsigned int* destination, const unsigned int* indices, size_t index_count, const float* vertex_positions_data, size_t vertex_count, size_t vertex_positions_stride, const float* vertex_attributes_data, size_t vertex_attributes_stride, const float* attribute_weights, size_t attribute_count, const unsigned char* vertex_lock, size_t target_index_count, float target_error, unsigned int options, float* out_result_error) {
ara_lod_allocator& allocator = p.allocator;
EdgeAdjacency& adjacency = p.adjacency;
unsigned int*& sparse_remap = p.sparse_remap;
unsigned int*& remap = p.remap;
unsigned int*& wedge = p.wedge;
unsigned char*& vertex_kind = p.vertex_kind;
unsigned int*& loop = p.loop;
unsigned int*& loopback = p.loopback;
Vector3*& vertex_positions = p.vertex_positions;
float& vertex_scale = p.vertex_scale;
float*& vertex_attributes = p.vertex_attributes;
Quadric*& vertex_quadrics = p.vertex_quadrics;
Quadric*& attribute_quadrics = p.attribute_quadrics;
QuadricGrad*& attribute_gradients = p.attribute_gradients;
QuadricGrad*& volume_gradients = p.volume_gradients;
unsigned int*& components = p.components;
float*& component_errors = p.component_errors;
size_t& component_count = p.component_count;
float& component_nexterror = p.component_nexterror;
size_t& collapse_capacity = p.collapse_capacity;
Collapse*& edge_collapses = p.edge_collapses;
unsigned int*& collapse_order = p.collapse_order;
unsigned int*& collapse_remap = p.collapse_remap;
unsigned char*& collapse_locked = p.collapse_locked;
float (&vertex_offset)[3] = p.vertex_offset;
unsigned int (&attribute_remap)[kMaxAttributes] = p.attribute_remap;

attribute_count=p.used_attributes;
unsigned int* result=destination;
bool first_iteration=true;
size_t result_count = index_count;
	float result_error = 0;
	float vertex_error = 0;

	// target_error input is linear; we need to adjust it to match quadricError units
	float error_scale = (options & meshopt_SimplifyErrorAbsolute) ? vertex_scale : 1.f;
	float error_limit = (target_error * target_error) / (error_scale * error_scale);

	while (result_count > target_index_count)
	{
		size_t edge_collapse_count = p.first_count;
if (!first_iteration) {
// note: throughout the simplification process adjacency structure reflects welded topology for result-in-progress
		updateEdgeAdjacency(adjacency, result, result_count, vertex_count, remap);

		edge_collapse_count = pickEdgeCollapses(edge_collapses, collapse_capacity, result, result_count, remap, vertex_kind, loop, loopback);
		assert(edge_collapse_count <= collapse_capacity);

		// no edges can be collapsed any more due to topology restrictions
		if (edge_collapse_count == 0)
			break;

#if TRACE
		printf("pass %d: ", int(pass_count++));
#endif

		rankEdgeCollapses(edge_collapses, edge_collapse_count, vertex_positions, vertex_attributes, vertex_quadrics, attribute_quadrics, attribute_gradients, attribute_count, remap, wedge, vertex_kind, loop, loopback, (options & meshopt_SimplifyErrorClamped) != 0);

		sortEdgeCollapses(collapse_order, edge_collapses, edge_collapse_count);


}
first_iteration=false;
if(edge_collapse_count==0) break;
size_t triangle_collapse_goal = (result_count - target_index_count) / 3;

		for (size_t i = 0; i < vertex_count; ++i)
			collapse_remap[i] = unsigned(i);

		memset(collapse_locked, 0, vertex_count);

		size_t collapses = performEdgeCollapses(collapse_remap, collapse_locked, edge_collapses, edge_collapse_count, collapse_order, remap, wedge, vertex_kind, loop, loopback, vertex_positions, adjacency, triangle_collapse_goal, error_limit, result_error);

		// no edges can be collapsed any more due to hitting the error limit or triangle collapse limit
		if (collapses == 0)
			break;

		updateQuadrics(collapse_remap, vertex_count, vertex_quadrics, volume_gradients, attribute_quadrics, attribute_gradients, attribute_count, vertex_positions, remap, vertex_error);

		// updateQuadrics will update vertex error if we use attributes, but if we don't then result_error and vertex_error are equivalent
		vertex_error = attribute_count == 0 ? result_error : vertex_error;

		// note: we update loops following edge collapses, but after this we might still have stale loop data
		// this can happen when a triangle with a loop edge gets collapsed along a non-loop edge
		// that works since a loop that points to a vertex that is no longer connected is not affecting collapse logic
		remapEdgeLoops(loop, vertex_count, collapse_remap);
		remapEdgeLoops(loopback, vertex_count, collapse_remap);

		result_count = remapIndexBuffer(result, result_count, collapse_remap, remap);

		if ((options & meshopt_SimplifyPrune) && result_count > target_index_count && component_nexterror <= vertex_error)
			result_count = pruneComponents(result, result_count, components, component_errors, component_count, vertex_error, component_nexterror);
	}

	// at this point, component_nexterror might be stale: component it references may have been removed through a series of edge collapses
	bool component_nextstale = true;

	// we're done with the regular simplification but we're still short of the target; try pruning more aggressively towards error_limit
	while ((options & meshopt_SimplifyPrune) && result_count > target_index_count && component_nexterror <= error_limit)
	{
#if TRACE
		printf("pass %d: cleanup; ", int(pass_count++));
#endif

		float component_cutoff = component_nexterror * 1.5f < error_limit ? component_nexterror * 1.5f : error_limit;

		// track maximum error in eligible components as we are increasing resulting error
		float component_maxerror = 0;
		for (size_t i = 0; i < component_count; ++i)
			if (component_errors[i] > component_maxerror && component_errors[i] <= component_cutoff)
				component_maxerror = component_errors[i];

		size_t new_count = pruneComponents(result, result_count, components, component_errors, component_count, component_cutoff, component_nexterror);
		if (new_count == result_count && !component_nextstale)
			break;

		component_nextstale = false; // pruneComponents guarantees next error is up to date
		result_count = new_count;
		result_error = result_error < component_maxerror ? component_maxerror : result_error;
		vertex_error = vertex_error < component_maxerror ? component_maxerror : vertex_error;
	}

#if TRACE
	printf("result: %d triangles, error: %e (pos %.3e); total %d passes\n", int(result_count / 3), sqrtf(result_error), sqrtf(vertex_error), int(pass_count));
#endif

	// if solve is requested, update input buffers destructively from internal data
	if (options & meshopt_SimplifyInternalSolve)
	{
		unsigned char* vertex_update = collapse_locked; // reuse as scratch space
		memset(vertex_update, 0, vertex_count);

		// limit quadric solve to vertices that are still used in the result
		for (size_t i = 0; i < result_count; ++i)
		{
			unsigned int v = result[i];

			// mark the vertex for finalizeVertices and root vertex for solve*
			vertex_update[remap[v]] = vertex_update[v] = 1;
		}

		// edge adjacency may be stale as we haven't updated it after last series of edge collapses
		updateEdgeAdjacency(adjacency, result, result_count, vertex_count, remap);

		solvePositions(vertex_positions, vertex_count, vertex_quadrics, volume_gradients, attribute_quadrics, attribute_gradients, attribute_count, remap, wedge, adjacency, vertex_kind, vertex_update);

		if (attribute_count)
			solveAttributes(vertex_positions, vertex_attributes, vertex_count, attribute_quadrics, attribute_gradients, attribute_count, remap, wedge, vertex_kind, vertex_update);

		finalizeVertices(const_cast<float*>(vertex_positions_data), vertex_positions_stride, const_cast<float*>(vertex_attributes_data), vertex_attributes_stride, attribute_weights, attribute_count, vertex_count, vertex_positions, vertex_attributes, sparse_remap, attribute_remap, vertex_scale, vertex_offset, vertex_kind, vertex_update, vertex_lock);
	}

	// if debug visualization data is requested, fill it instead of index data; for simplicity, this doesn't work with sparsity
	if ((options & meshopt_SimplifyInternalDebug) && !sparse_remap)
	{
		assert(Kind_Count <= 8 && vertex_count < (1 << 28)); // 3 bit kind, 1 bit loop

		for (size_t i = 0; i < result_count; i += 3)
		{
			unsigned int a = result[i + 0], b = result[i + 1], c = result[i + 2];

			result[i + 0] |= (vertex_kind[a] << 28) | (unsigned(loop[a] == b || loopback[b] == a) << 31);
			result[i + 1] |= (vertex_kind[b] << 28) | (unsigned(loop[b] == c || loopback[c] == b) << 31);
			result[i + 2] |= (vertex_kind[c] << 28) | (unsigned(loop[c] == a || loopback[a] == c) << 31);
		}
	}

	// convert resulting indices back into the dense space of the larger mesh
	if (sparse_remap)
		for (size_t i = 0; i < result_count; ++i)
			result[i] = sparse_remap[result[i]];

	// result_error is quadratic; we need to remap it back to linear
	if (out_result_error)
		*out_result_error = sqrtf(result_error) * error_scale;

	return result_count;

}
}
