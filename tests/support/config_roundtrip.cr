require "../../../obsctl/src/obsctl/config/config"
require "../../../obsctl/src/obsctl/config/config_schema"

config = Obsctl::Config::Config.from_yaml(File.read(ARGV[0]))
Obsctl::Config::ConfigSchema.validate!(config)
File.write(ARGV[1], config.to_yaml)
