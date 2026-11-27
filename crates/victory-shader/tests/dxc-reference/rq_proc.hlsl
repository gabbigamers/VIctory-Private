// Reference shader written here: inline RayQuery over procedural (AABB) primitives, the shader commits the hit itself.
struct Ray { float3 o; float tmin; float3 d; float tmax; uint mask; uint flags; uint r0; uint r1; };
struct Hit { float t; float u; float v; uint kind; uint inst_index; uint inst_id; uint geom; uint prim; };
RaytracingAccelerationStructure tlas : register(t0);
StructuredBuffer<Ray> rays : register(t1);
ByteAddressBuffer verts : register(t2);
RWStructuredBuffer<Hit> hits : register(u0);
[numthreads(64,1,1)]
void main(uint3 id : SV_DispatchThreadID) {
  Ray r = rays[id.x];
  RayDesc rd; rd.Origin = r.o; rd.TMin = r.tmin; rd.Direction = r.d; rd.TMax = r.tmax;
  RayQuery<RAY_FLAG_NONE> q;
  q.TraceRayInline(tlas, r.flags, r.mask, rd);
  float bu = 0, bv = 0; uint face = 0;
  while (q.Proceed()) {
    if (q.CandidateType() == CANDIDATE_PROCEDURAL_PRIMITIVE) {
      uint p = q.CandidatePrimitiveIndex();
      float3 a = asfloat(verts.Load3(p * 36)), b = asfloat(verts.Load3(p * 36 + 12)), c = asfloat(verts.Load3(p * 36 + 24));
      float3 o = q.CandidateObjectRayOrigin(), d = q.CandidateObjectRayDirection();
      float3 e1 = b - a, e2 = c - a, pv = cross(d, e2);
      float det = dot(e1, pv);
      if (det != 0) {
        float3 tv = o - a; float u = dot(tv, pv) / det; float3 qv = cross(tv, e1);
        float v = dot(d, qv) / det; float t = dot(e2, qv) / det;
        if (u >= 0 && v >= 0 && u + v <= 1 && t >= q.RayTMin() && t <= q.CommittedRayT()) {
          q.CommitProceduralPrimitiveHit(t); bu = u; bv = v; face = det > 0 ? 1 : 2;
        }
      }
    }
  }
  Hit h = (Hit)0;
  if (q.CommittedStatus() == COMMITTED_PROCEDURAL_PRIMITIVE_HIT) {
    h.t = q.CommittedRayT(); h.u = bu; h.v = bv; h.kind = 3;
    h.inst_index = q.CommittedInstanceIndex(); h.inst_id = q.CommittedInstanceID();
    h.geom = q.CommittedGeometryIndex(); h.prim = q.CommittedPrimitiveIndex();
  }
  hits[id.x] = h;
}
