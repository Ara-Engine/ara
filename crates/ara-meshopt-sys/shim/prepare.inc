// Derived from meshoptimizer 1.3, Copyright (c) 2016-2026 Arseny Kapoulkine.
// MIT notice: ../vendor/LICENSE. Arithmetic/traversal order remains upstream.
namespace ara_lod {
static void prepare(State& p, unsigned int* destination, const unsigned int* indices, size_t index_count, const float* vertex_positions_data, size_t vertex_count, size_t vertex_positions_stride, const float* vertex_attributes_data, size_t vertex_attributes_stride, const float* attribute_weights, size_t attribute_count, const unsigned char* vertex_lock, size_t target_index_count, float target_error, unsigned int options, float* out_result_error) {
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



	unsigned int* result = destination;
	if (result != indices)
		memcpy(result, indices, index_count * sizeof(unsigned int));

	// build an index remap and update indices/vertex_count to minimize the subsequent work
	// note: as a consequence, errors will be computed relative to the subset extent
	sparse_remap = NULL;
	if (options & meshopt_SimplifySparse)
		sparse_remap = buildSparseRemap(result, index_count, vertex_count, &vertex_count, allocator);

	// build adjacency information
	adjacency = {};
	prepareEdgeAdjacency(adjacency, index_count, vertex_count, allocator);
	updateEdgeAdjacency(adjacency, result, index_count, vertex_count, NULL);

	// build position remap that maps each vertex to the one with identical position
	// wedge table stores next vertex with identical position for each vertex
	remap = allocator.allocate<unsigned int>(vertex_count);
	wedge = allocator.allocate<unsigned int>(vertex_count);
	buildPositionRemap(remap, wedge, vertex_positions_data, vertex_count, vertex_positions_stride, sparse_remap, allocator);

	// classify vertices; vertex kind determines collapse rules, see kCanCollapse
	vertex_kind = allocator.allocate<unsigned char>(vertex_count);
	loop = allocator.allocate<unsigned int>(vertex_count);
	loopback = allocator.allocate<unsigned int>(vertex_count);
	classifyVertices(vertex_kind, loop, loopback, vertex_count, adjacency, remap, wedge, vertex_lock, sparse_remap, options);

#if TRACE
	size_t unique_positions = 0;
	for (size_t i = 0; i < vertex_count; ++i)
		unique_positions += remap[i] == i;

	printf("position remap: %d vertices => %d positions\n", int(vertex_count), int(unique_positions));

	size_t kinds[Kind_Count] = {};
	for (size_t i = 0; i < vertex_count; ++i)
		kinds[vertex_kind[i]] += remap[i] == i;

	printf("kinds: manifold %d, border %d, seam %d, complex %d, fringe %d, locked %d\n",
	    int(kinds[Kind_Manifold]), int(kinds[Kind_Border]), int(kinds[Kind_Seam]), int(kinds[Kind_Complex]), int(kinds[Kind_Fringe]), int(kinds[Kind_Locked]));
#endif

	vertex_positions = allocator.allocate<Vector3>(vertex_count);

	vertex_scale = rescalePositions(vertex_positions, vertex_positions_data, vertex_count, vertex_positions_stride, sparse_remap, vertex_offset);

	vertex_attributes = NULL;


	if (attribute_count)
	{
		// remap attributes to only include ones with weight > 0 to minimize memory/compute overhead for quadrics
		size_t attributes_used = 0;
		for (size_t i = 0; i < attribute_count; ++i)
			if (attribute_weights[i] > 0)
				attribute_remap[attributes_used++] = unsigned(i);

		attribute_count = attributes_used;
		vertex_attributes = allocator.allocate<float>(vertex_count * attribute_count);
		rescaleAttributes(vertex_attributes, vertex_attributes_data, vertex_count, vertex_attributes_stride, attribute_weights, attribute_count, attribute_remap, sparse_remap);
	}

	vertex_quadrics = allocator.allocate<Quadric>(vertex_count);
	memset(vertex_quadrics, 0, vertex_count * sizeof(Quadric));

	attribute_quadrics = NULL;
	attribute_gradients = NULL;
	volume_gradients = NULL;

	if (attribute_count)
	{
		attribute_quadrics = allocator.allocate<Quadric>(vertex_count);
		memset(attribute_quadrics, 0, vertex_count * sizeof(Quadric));

		attribute_gradients = allocator.allocate<QuadricGrad>(vertex_count * attribute_count);
		memset(attribute_gradients, 0, vertex_count * attribute_count * sizeof(QuadricGrad));

		if (options & meshopt_SimplifyInternalSolve)
		{
			volume_gradients = allocator.allocate<QuadricGrad>(vertex_count);
			memset(volume_gradients, 0, vertex_count * sizeof(QuadricGrad));
		}
	}

	fillFaceQuadrics(vertex_quadrics, volume_gradients, result, index_count, vertex_positions, remap);
	fillVertexQuadrics(vertex_quadrics, vertex_positions, vertex_count, remap, vertex_lock, sparse_remap, options);
	fillEdgeQuadrics(vertex_quadrics, result, index_count, vertex_positions, remap, vertex_kind, loop, loopback);

	if (options & meshopt_SimplifyPreserveFolds)
		fillFoldQuadrics(vertex_quadrics, adjacency, vertex_positions, vertex_count, remap, wedge);

	if (attribute_count)
		fillAttributeQuadrics(attribute_quadrics, attribute_gradients, result, index_count, vertex_positions, vertex_attributes, attribute_count);

	components = NULL;
	component_errors = NULL;
	component_count = 0;
	component_nexterror = 0;

	if (options & meshopt_SimplifyPrune)
	{
		components = allocator.allocate<unsigned int>(vertex_count);
		component_count = buildComponents(components, vertex_count, result, index_count, remap);

		component_errors = allocator.allocate<float>(component_count * 4); // overallocate for temporary use inside measureComponents
		measureComponents(component_errors, component_count, components, vertex_positions, vertex_count);

		component_nexterror = FLT_MAX;
		for (size_t i = 0; i < component_count; ++i)
			component_nexterror = component_nexterror > component_errors[i] ? component_errors[i] : component_nexterror;

#if TRACE
		printf("components: %d (min error %e)\n", int(component_count), sqrtf(component_nexterror));
#endif
	}

#if TRACE
	size_t pass_count = 0;
#endif

	collapse_capacity = boundEdgeCollapses(adjacency, vertex_count, index_count, vertex_kind);

	edge_collapses = allocator.allocate<Collapse>(collapse_capacity);
	collapse_order = allocator.allocate<unsigned int>(collapse_capacity);
	collapse_remap = allocator.allocate<unsigned int>(vertex_count);
	collapse_locked = allocator.allocate<unsigned char>(vertex_count);


// note: throughout the simplification process adjacency structure reflects welded topology for result-in-progress
		updateEdgeAdjacency(adjacency, result, index_count, vertex_count, remap);

		size_t edge_collapse_count = pickEdgeCollapses(edge_collapses, collapse_capacity, result, index_count, remap, vertex_kind, loop, loopback);
		assert(edge_collapse_count <= collapse_capacity);

		// no edges can be collapsed any more due to topology restrictions
		if (edge_collapse_count != 0) {

#if TRACE
		printf("pass %d: ", int(pass_count++));
#endif

		rankEdgeCollapses(edge_collapses, edge_collapse_count, vertex_positions, vertex_attributes, vertex_quadrics, attribute_quadrics, attribute_gradients, attribute_count, remap, wedge, vertex_kind, loop, loopback, (options & meshopt_SimplifyErrorClamped) != 0);

		sortEdgeCollapses(collapse_order, edge_collapses, edge_collapse_count);

		}
 p.first_count=edge_collapse_count;

p.used_attributes=attribute_count;
p.save(adjacency.offsets,(vertex_count+1)*sizeof(unsigned int));
p.save(adjacency.data,index_count*sizeof(EdgeAdjacency::Edge));
p.save(loop,vertex_count*sizeof(unsigned int));
p.save(loopback,vertex_count*sizeof(unsigned int));
p.save(vertex_quadrics,vertex_count*sizeof(Quadric));
p.save(attribute_quadrics,vertex_count*sizeof(Quadric));
p.save(attribute_gradients,vertex_count*attribute_count*sizeof(QuadricGrad));
p.save(edge_collapses,p.first_count*sizeof(Collapse));
p.save(collapse_order,p.first_count*sizeof(unsigned int));
}
}
