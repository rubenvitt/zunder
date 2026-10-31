#!/usr/bin/env ruby
# Adds the share extension to the Xcode project of `tauri ios init`
# (src-tauri/gen/apple, not checked in): a target ZunderShare embedded in the
# app, and the App Group for both. Run after `tauri ios init`, then
# `xcodegen generate` in gen/apple:
#   ruby share/add-ios-extension.rb src-tauri/gen/apple <version>
require "pathname"
require "yaml"

APP_GROUP = "group.dev.rubeen.zunder"

project_dir = Pathname.new(ARGV.fetch(0)).realpath
version = ARGV.fetch(1)
spec_path = project_dir.join("project.yml")
share = Pathname.new(__dir__).realpath
rel = ->(path) { share.join(path).relative_path_from(project_dir).to_s }

spec = YAML.safe_load(spec_path.read, aliases: true)
targets = spec.fetch("targets")
app_name = targets.keys.find { |name| name.end_with?("_iOS") } or abort "Kein iOS-Ziel in #{spec_path}"
app = targets.fetch(app_name)
deployment = spec.dig("options", "deploymentTarget", "iOS")
team = spec.dig("settingGroups", "app", "base", "DEVELOPMENT_TEAM")

# The archive in the CI runs without signing (CODE_SIGNING_ALLOWED=NO) and
# Xcode signs on export with the entitlements already in the binary; the
# extension therefore signs itself ad hoc with its entitlements, as Tauri
# does for the app.
adhoc = <<~SH
  if [ "${CODE_SIGNING_ALLOWED:-YES}" = "NO" ]; then
    codesign --force --sign - --entitlements "${SRCROOT:?}/#{rel.call("iOS/ZunderShare.entitlements")}" "${CODESIGNING_FOLDER_PATH:?}"
  fi
SH

settings = {
  "PRODUCT_NAME" => "ZunderShare",
  "PRODUCT_BUNDLE_IDENTIFIER" => "dev.rubeen.zunder.share",
  "INFOPLIST_FILE" => rel.call("iOS/Info.plist"),
  "CODE_SIGN_ENTITLEMENTS" => rel.call("iOS/ZunderShare.entitlements"),
  "MARKETING_VERSION" => version,
  "CURRENT_PROJECT_VERSION" => version,
  "SWIFT_VERSION" => "5.0",
  "TARGETED_DEVICE_FAMILY" => "1,2",
  "SKIP_INSTALL" => "YES",
}
settings["DEVELOPMENT_TEAM"] = team if team

targets["ZunderShare"] = {
  "type" => "app-extension",
  "platform" => "iOS",
  "deploymentTarget" => deployment,
  "sources" => [
    { "path" => rel.call("Shared") },
    { "path" => rel.call("iOS"), "excludes" => ["*.plist", "*.entitlements"] },
  ],
  "settings" => { "base" => settings },
  "dependencies" => [
    { "sdk" => "Social.framework" },
    { "sdk" => "UIKit.framework" },
    { "sdk" => "UniformTypeIdentifiers.framework" },
  ],
  "postBuildScripts" => [
    { "name" => "Ad-hoc signieren ohne Signatur", "script" => adhoc, "basedOnDependencyAnalysis" => false },
  ],
}.compact

deps = (app["dependencies"] ||= [])
deps << { "target" => "ZunderShare", "embed" => true } unless deps.any? { |d| d["target"] == "ZunderShare" }

entitlements = (app["entitlements"] ||= { "path" => "#{app_name}/#{app_name}.entitlements" })
props = (entitlements["properties"] ||= {})
groups = (props["com.apple.security.application-groups"] ||= [])
groups << APP_GROUP unless groups.include?(APP_GROUP)

spec_path.write(YAML.dump(spec))
puts "Teilen-Erweiterung in #{spec_path} eingetragen (#{app_name}, Version #{version})"
