output "url" {
  value       = "https://${azurerm_container_app.this.ingress[0].fqdn}/health"
  description = "Public HTTPS URL (health endpoint)."
}

output "fqdn" {
  value = azurerm_container_app.this.ingress[0].fqdn
}
