output "url" {
  value       = google_cloud_run_v2_service.this.uri
  description = "Public HTTPS URL of the Cloud Run service. Append /health to check."
}

output "service_name" {
  value = google_cloud_run_v2_service.this.name
}
