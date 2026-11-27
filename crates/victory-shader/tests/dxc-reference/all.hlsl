RaytracingAccelerationStructure tlas : register(t0, space3);
RWByteAddressBuffer o : register(u0);
[numthreads(64,1,1)]
void main(uint3 id : SV_DispatchThreadID) {
  RayDesc rd; rd.Origin = float3(id.x,0,0); rd.TMin = 0; rd.Direction = float3(0,0,1); rd.TMax = 100;
  RayQuery<RAY_FLAG_NONE> q;
  q.TraceRayInline(tlas, id.y, id.z, rd);
  float acc = 0; uint ia = 0;
  while (q.Proceed()) {
    if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
      float2 b = q.CandidateTriangleBarycentrics();
      acc += b.x + b.y + q.CandidateTriangleRayT();
      ia += q.CandidateTriangleFrontFace() ? 1 : 0;
      q.CommitNonOpaqueTriangleHit();
    } else {
      ia += q.CandidateProceduralPrimitiveNonOpaque() ? 1 : 0;
      q.CommitProceduralPrimitiveHit(1.0);
    }
    float3x4 m = q.CandidateObjectToWorld3x4(); float3x4 n = q.CandidateWorldToObject3x4();
    acc += m[0][0] + m[2][3] + n[1][2];
    acc += dot(q.CandidateObjectRayOrigin(), q.CandidateObjectRayDirection());
    ia += q.CandidateInstanceIndex() + q.CandidateInstanceID() + q.CandidateGeometryIndex() + q.CandidatePrimitiveIndex() + q.CandidateInstanceContributionToHitGroupIndex();
    if (ia > 100) q.Abort();
  }
  ia += q.CommittedStatus() + q.RayFlags();
  acc += q.RayTMin() + q.CommittedRayT();
  acc += dot(q.WorldRayOrigin(), q.WorldRayDirection());
  float3x4 m = q.CommittedObjectToWorld3x4(); float3x4 n = q.CommittedWorldToObject3x4();
  acc += m[0][0] + m[2][3] + n[1][2];
  acc += dot(q.CommittedObjectRayOrigin(), q.CommittedObjectRayDirection());
  float2 b = q.CommittedTriangleBarycentrics(); acc += b.x + b.y;
  ia += q.CommittedTriangleFrontFace() ? 1 : 0;
  ia += q.CommittedInstanceIndex() + q.CommittedInstanceID() + q.CommittedGeometryIndex() + q.CommittedPrimitiveIndex() + q.CommittedInstanceContributionToHitGroupIndex();
  o.Store2(id.x * 8, uint2(asuint(acc), ia));
}
