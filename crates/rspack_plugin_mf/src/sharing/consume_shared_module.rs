use std::{borrow::Cow, hash::Hash};

use async_trait::async_trait;
use rspack_core::{
  async_module_factory, impl_build_info_meta, rspack_sources::Source, sync_module_factory,
  AsyncDependenciesBlock, AsyncDependenciesBlockId, BoxDependency, BuildContext, BuildInfo,
  BuildMeta, BuildResult, CodeGenerationResult, Compilation, Context, DependenciesBlock,
  DependencyId, LibIdentOptions, Module, ModuleIdentifier, ModuleType, RuntimeGlobals, RuntimeSpec,
  SourceType,
};
use rspack_error::{impl_empty_diagnosable_trait, Result};
use rspack_hash::RspackHash;
use rspack_identifier::{Identifiable, Identifier};

use super::{
  consume_shared_fallback_dependency::ConsumeSharedFallbackDependency,
  consume_shared_runtime_module::CodeGenerationDataConsumeShared,
};
use crate::{utils::json_stringify, ConsumeOptions};

#[derive(Debug)]
pub struct ConsumeSharedModule {
  blocks: Vec<AsyncDependenciesBlockId>,
  dependencies: Vec<DependencyId>,
  identifier: ModuleIdentifier,
  lib_ident: String,
  readable_identifier: String,
  context: Context,
  options: ConsumeOptions,
  build_info: Option<BuildInfo>,
  build_meta: Option<BuildMeta>,
}

impl ConsumeSharedModule {
  pub fn new(context: Context, options: ConsumeOptions) -> Self {
    let identifier = format!(
      "consume shared module ({}) {}@{}{}{}{}{}",
      &options.share_scope,
      &options.share_key,
      options
        .required_version
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "*".to_string()),
      options
        .strict_version
        .then_some(" (strict)")
        .unwrap_or_default(),
      options.singleton.then_some(" (strict)").unwrap_or_default(),
      options
        .import_resolved
        .as_ref()
        .map(|f| format!(" (fallback: {f})"))
        .unwrap_or_default(),
      options.eager.then_some(" (eager)").unwrap_or_default(),
    );
    Self {
      blocks: Vec::new(),
      dependencies: Vec::new(),
      identifier: ModuleIdentifier::from(identifier.clone()),
      lib_ident: format!(
        "webpack/sharing/consume/{}/{}{}",
        &options.share_scope,
        &options.share_key,
        options
          .import
          .as_ref()
          .map(|r| format!("/{}", r))
          .unwrap_or_default()
      ),
      readable_identifier: identifier,
      context,
      options,
      build_info: None,
      build_meta: None,
    }
  }
}

impl Identifiable for ConsumeSharedModule {
  fn identifier(&self) -> Identifier {
    self.identifier
  }
}

impl DependenciesBlock for ConsumeSharedModule {
  fn add_block_id(&mut self, block: AsyncDependenciesBlockId) {
    self.blocks.push(block)
  }

  fn get_blocks(&self) -> &[AsyncDependenciesBlockId] {
    &self.blocks
  }

  fn add_dependency_id(&mut self, dependency: DependencyId) {
    self.dependencies.push(dependency)
  }

  fn get_dependencies(&self) -> &[DependencyId] {
    &self.dependencies
  }
}

#[async_trait]
impl Module for ConsumeSharedModule {
  impl_build_info_meta!();

  fn size(&self, _source_type: &SourceType) -> f64 {
    42.0
  }

  fn module_type(&self) -> &ModuleType {
    &ModuleType::ConsumeShared
  }

  fn source_types(&self) -> &[SourceType] {
    &[SourceType::ConsumeShared]
  }

  fn original_source(&self) -> Option<&dyn Source> {
    None
  }

  fn readable_identifier(&self, _context: &Context) -> Cow<str> {
    self.readable_identifier.as_str().into()
  }

  fn lib_ident(&self, _options: LibIdentOptions) -> Option<Cow<str>> {
    Some(self.lib_ident.as_str().into())
  }

  fn get_context(&self) -> Option<Box<Context>> {
    Some(Box::new(self.context.clone()))
  }

  async fn build(&mut self, build_context: BuildContext<'_>) -> Result<BuildResult> {
    let mut hasher = RspackHash::from(&build_context.compiler_options.output);
    self.update_hash(&mut hasher);
    let hash = hasher.digest(&build_context.compiler_options.output.hash_digest);

    let mut blocks = vec![];
    let mut dependencies = vec![];
    if let Some(fallback) = &self.options.import {
      let dep = Box::new(ConsumeSharedFallbackDependency::new(fallback.to_owned()));
      if self.options.eager {
        dependencies.push(dep as BoxDependency);
      } else {
        let mut block = AsyncDependenciesBlock::new(self.identifier, None);
        block.add_dependency(dep);
        blocks.push(block);
      }
    }

    Ok(BuildResult {
      build_info: BuildInfo {
        hash: Some(hash),
        ..Default::default()
      },
      build_meta: Default::default(),
      dependencies,
      blocks,
      ..Default::default()
    })
  }

  #[allow(clippy::unwrap_in_result)]
  fn code_generation(
    &self,
    compilation: &Compilation,
    _runtime: Option<&RuntimeSpec>,
  ) -> Result<CodeGenerationResult> {
    let mut code_generation_result = CodeGenerationResult::default();
    code_generation_result
      .runtime_requirements
      .insert(RuntimeGlobals::SHARE_SCOPE_MAP);
    let mut function = String::from("loaders.load");
    let mut args = vec![
      json_stringify(&self.options.share_scope),
      json_stringify(&self.options.share_key),
    ];
    if let Some(version) = &self.options.required_version {
      if self.options.strict_version {
        function += "Strict";
      }
      if self.options.singleton {
        function += "Singleton";
      }
      let version = json_stringify(&version.to_string());
      args.push(format!("loaders.parseRange({})", version));
      function += "VersionCheck";
    } else if self.options.singleton {
      function += "Singleton";
    }
    let factory = self.options.import.as_ref().map(|fallback| {
      if self.options.eager {
        sync_module_factory(
          &self.get_dependencies()[0],
          fallback,
          compilation,
          &mut code_generation_result.runtime_requirements,
        )
      } else {
        async_module_factory(
          &self.get_blocks()[0],
          fallback,
          compilation,
          &mut code_generation_result.runtime_requirements,
        )
      }
    });
    code_generation_result
      .data
      .insert(CodeGenerationDataConsumeShared {
        share_scope: self.options.share_scope.clone(),
        share_key: self.options.share_key.clone(),
        import: self.options.import.clone(),
        required_version: self.options.required_version.clone(),
        strict_version: self.options.strict_version,
        singleton: self.options.singleton,
        eager: self.options.eager,
        fallback: factory,
      });
    Ok(code_generation_result)
  }
}

impl_empty_diagnosable_trait!(ConsumeSharedModule);

impl Hash for ConsumeSharedModule {
  fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
    "__rspack_internal__ConsumeSharedModule".hash(state);
    self.identifier().hash(state);
  }
}

impl PartialEq for ConsumeSharedModule {
  fn eq(&self, other: &Self) -> bool {
    self.identifier() == other.identifier()
  }
}

impl Eq for ConsumeSharedModule {}
